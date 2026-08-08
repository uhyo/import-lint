//! The rule engine (spec §3–§4, M3): for every checked entry in every lint target,
//! resolve its one-hop export-table lookup (falling through star-export chains when
//! needed), then decide pass/fail from the resolved [`Access`] level.

mod in_package;
pub mod options;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use oxc_str::CompactStr;

use crate::diagnostics::{Diagnostic, MessageId};
use crate::extract::{Access, CheckedEntry, EntryKind, ExportInfo, FileModuleInfo};
use crate::graph::ModuleGraph;
use crate::resolve::Provenance;

pub use in_package::{CompiledPackageOptions, compile_package_directory_patterns, is_in_package};
pub use options::{
    Importability, NonTsFilesEntry, NonTsFilesOption, PackageAccessRuleOptions, SelfRefOpt,
};

/// The rule name suppression directives match this engine's diagnostics under —
/// the same name the config file and rendered output use for the rule.
pub const PACKAGE_ACCESS_RULE_NAME: &str = "package-access";

/// Run the rule engine over every lint target in `graph`, producing every violation
/// under `options`. `project_root` anchors `packageDirectory` and
/// `excludeSourcePatterns` glob matching (both match against paths relative to it).
pub fn check_graph(
    graph: &ModuleGraph,
    options: &PackageAccessRuleOptions,
    project_root: &Path,
) -> Vec<Diagnostic> {
    let targets: Vec<&Path> = graph.lint_targets.iter().map(PathBuf::as_path).collect();
    check_files(graph, options, project_root, &targets)
}

/// Same as [`check_graph`], but scoped to `files` instead of every lint target in
/// `graph`. Watch mode's incremental fast path (`crates/cli/src/watch.rs`, PLAN-v1.md
/// §7) calls this with just the dirty set — the changed files plus, if their export
/// surface changed, their importers and star-export closure — so a single-file edit
/// doesn't re-check the whole project. Each file's diagnostics depend only on its own
/// `checked_entries`/resolutions and the one-hop-reachable export tables (never on any
/// other lint target's diagnostics), so checking a subset is exactly as correct as
/// checking everything and discarding the rest.
pub fn check_files(
    graph: &ModuleGraph,
    options: &PackageAccessRuleOptions,
    project_root: &Path,
    files: &[&Path],
) -> Vec<Diagnostic> {
    let package_directory = options
        .package_directory
        .as_ref()
        .map(|patterns| compile_package_directory_patterns(patterns));
    let package_options = CompiledPackageOptions {
        index_loophole: options.index_loophole,
        filename_loophole: options.filename_loophole,
        package_directory,
        project_directory: project_root.to_path_buf(),
    };

    let exclude_patterns: Vec<globset::GlobMatcher> = options
        .exclude_source_patterns
        .iter()
        .filter_map(|pattern| {
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    eprintln!(
                        "import-lint: invalid excludeSourcePatterns pattern '{pattern}': {err}, ignoring"
                    );
                })
                .ok()
        })
        .collect();

    let non_ts_entries = compile_non_ts_entries(&options.non_ts_files);

    let default_access = importability_to_access(options.default_importability);

    let mut diagnostics = Vec::new();

    for &importer in files {
        let Some(file) = graph.file(importer) else {
            continue;
        };
        for entry in &file.checked_entries {
            let Some(provenance) = graph.resolution(importer, &entry.specifier) else {
                continue;
            };
            let Provenance::Internal(target) = provenance else {
                continue;
            };

            // A non-TS target (CSS module, JSON, ...) has no export table: any
            // imported name is assumed to exist, with its access level taken from
            // the `nonTsFiles` option (else `defaultImportability`, like a TS
            // export with no JSDoc tag).
            let resolved = if graph.non_ts_files.contains(target) {
                let name: &str = if entry.kind == EntryKind::ImportDefault {
                    "default"
                } else {
                    entry.imported_name.as_str()
                };
                let relative = in_package::node_relative(project_root, target);
                let access = non_ts_access(&non_ts_entries, &relative, name);
                Some((target.clone(), access, CompactStr::from(name)))
            } else {
                lookup(graph, target, entry)
                    .map(|(path, info, identifier)| (path, info.access, identifier))
            };
            let Some((exporter_path, declared_access, identifier)) = resolved else {
                continue;
            };

            if !exclude_patterns.is_empty() {
                let relative = in_package::node_relative(project_root, &exporter_path);
                if exclude_patterns.iter().any(|m| m.is_match(&relative)) {
                    continue;
                }
            }

            let access = declared_access.unwrap_or(default_access);
            let message_id = match access {
                Access::Public => continue,
                Access::Private => {
                    if entry.kind == EntryKind::ReExport {
                        MessageId::PrivateReexport
                    } else {
                        MessageId::Private
                    }
                }
                Access::Package => {
                    if is_in_package(importer, &exporter_path, &package_options) {
                        continue;
                    }
                    if entry.kind == EntryKind::ReExport {
                        MessageId::PackageReexport
                    } else {
                        MessageId::Package
                    }
                }
            };

            // Caller-side suppression directives (`import-lint-disable-next-line`
            // / `import-lint-disable-line` in the importer file) silence the
            // violation at this entry's line.
            if file
                .suppressions
                .iter()
                .any(|s| s.suppresses(entry.span.start, PACKAGE_ACCESS_RULE_NAME))
            {
                continue;
            }

            diagnostics.push(Diagnostic {
                path: importer.to_path_buf(),
                span: entry.span,
                message_id,
                identifier,
            });
        }
    }

    diagnostics.sort_by(|a, b| a.path.cmp(&b.path).then(a.span.start.cmp(&b.span.start)));
    diagnostics
}

fn importability_to_access(importability: Importability) -> Access {
    match importability {
        Importability::Public => Access::Public,
        Importability::Package => Access::Package,
        Importability::Private => Access::Private,
    }
}

/// One compiled `nonTsFiles` entry: the glob matcher over project-relative
/// resolved paths, plus a borrow of the entry's export-name -> access map.
struct CompiledNonTsEntry<'a> {
    matcher: globset::GlobMatcher,
    exports: &'a std::collections::HashMap<String, Importability>,
}

/// Compile the `nonTsFiles` option's glob patterns once per `check_files` call,
/// preserving config order. A pattern that fails to compile is dropped with a
/// stderr note (same policy as `excludeSourcePatterns`).
fn compile_non_ts_entries(option: &NonTsFilesOption) -> Vec<CompiledNonTsEntry<'_>> {
    option
        .entries
        .iter()
        .filter_map(|entry| {
            GlobBuilder::new(&entry.pattern)
                .literal_separator(true)
                .build()
                .map(|glob| CompiledNonTsEntry {
                    matcher: glob.compile_matcher(),
                    exports: &entry.exports,
                })
                .map_err(|err| {
                    eprintln!(
                        "import-lint: invalid nonTsFiles pattern '{}': {err}, ignoring",
                        entry.pattern
                    );
                })
                .ok()
        })
        .collect()
}

/// The access level `nonTsFiles` assigns to export `name` of the non-TS file at
/// `relative_path` (project-relative), or `None` if no entry assigns it (the
/// caller falls back to `defaultImportability`). Entries are tried in config
/// order; within a matching entry an exact name wins over `"*"`, and `"*"` never
/// matches `default` (the ES spec's `export *` convention).
fn non_ts_access(
    entries: &[CompiledNonTsEntry<'_>],
    relative_path: &str,
    name: &str,
) -> Option<Access> {
    for entry in entries {
        if !entry.matcher.is_match(relative_path) {
            continue;
        }
        if let Some(&importability) = entry.exports.get(name) {
            return Some(importability_to_access(importability));
        }
        if name != "default"
            && let Some(&importability) = entry.exports.get("*")
        {
            return Some(importability_to_access(importability));
        }
    }
    None
}

/// Resolve one checked entry's one-hop lookup target: the exporter file, its
/// `ExportInfo`, and the identifier to report (equal to `entry.imported_name`,
/// except when a default import falls through to a TS `export =`, where it becomes
/// `"export="`).
///
/// "One hop" means: only `target`'s own export table (or, transitively, files
/// reachable purely through `target`'s `star_exports` chain) is consulted. If the
/// matched entry is itself a passthrough re-export, its own `access` — not
/// whatever it re-exports — governs; we never hop a second time.
fn lookup<'g>(
    graph: &'g ModuleGraph,
    target: &Path,
    entry: &CheckedEntry,
) -> Option<(PathBuf, &'g ExportInfo, CompactStr)> {
    let file = graph.file(target)?;

    if entry.kind == EntryKind::ImportDefault {
        if let Some(info) = file.export_table.get("default") {
            return Some((target.to_path_buf(), info, CompactStr::from("default")));
        }
        // `export=` is a direct-table-only fallback for default imports; it never
        // flows through a star-export chain (checked below).
        if let Some(info) = file.export_table.get("export=") {
            return Some((target.to_path_buf(), info, CompactStr::from("export=")));
        }
    } else if let Some(info) = file.export_table.get(entry.imported_name.as_str()) {
        return Some((target.to_path_buf(), info, entry.imported_name.clone()));
    }

    let search_name: &str = if entry.kind == EntryKind::ImportDefault {
        "default"
    } else {
        entry.imported_name.as_str()
    };

    let mut visited = HashSet::new();
    visited.insert(target.to_path_buf());
    descend_star_exports(graph, target, search_name, &mut visited)
}

/// Depth-first, cycle-guarded descent through `file_path`'s `star_exports` (and,
/// transitively, each star-exported file's own `star_exports`), in source order,
/// looking for `search_name` in each descended file's direct export table. First
/// hit wins.
fn descend_star_exports<'g>(
    graph: &'g ModuleGraph,
    file_path: &Path,
    search_name: &str,
    visited: &mut HashSet<PathBuf>,
) -> Option<(PathBuf, &'g ExportInfo, CompactStr)> {
    let file: &FileModuleInfo = graph.file(file_path)?;

    for star_specifier in &file.star_exports {
        let Some(Provenance::Internal(next)) = graph.resolution(file_path, star_specifier) else {
            continue;
        };
        if !visited.insert(next.clone()) {
            continue;
        }

        if let Some(next_file) = graph.file(next)
            && let Some(info) = next_file.export_table.get(search_name)
        {
            return Some((next.clone(), info, CompactStr::from(search_name)));
        }

        if let Some(found) = descend_star_exports(graph, next, search_name, visited) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use oxc_span::Span;

    use super::*;
    use crate::extract::FileModuleInfo;

    fn entry(kind: EntryKind, imported_name: &str, specifier: &str, start: u32) -> CheckedEntry {
        CheckedEntry {
            kind,
            imported_name: CompactStr::from(imported_name),
            specifier: CompactStr::from(specifier),
            span: Span::new(start, start + 1),
        }
    }

    fn importer(path: &str, entries: Vec<CheckedEntry>) -> Arc<FileModuleInfo> {
        let specifiers = entries.iter().map(|e| e.specifier.clone()).collect();
        Arc::new(FileModuleInfo {
            path: PathBuf::from(path),
            checked_entries: entries,
            export_table: HashMap::new(),
            star_exports: Vec::new(),
            ambient_modules: Vec::new(),
            specifiers,
            suppressions: Vec::new(),
        })
    }

    /// Build a graph where every one of `importer_file`'s specifiers resolves to
    /// the target file mapped for it in `targets` (all internal).
    fn graph(importer_file: Arc<FileModuleInfo>, targets: &[(&str, &str)]) -> ModuleGraph {
        let mut resolutions = HashMap::new();
        for (specifier, target) in targets {
            resolutions.insert(
                (importer_file.path.clone(), CompactStr::from(*specifier)),
                Provenance::Internal(PathBuf::from(target)),
            );
        }
        let lint_targets = HashSet::from([importer_file.path.clone()]);
        ModuleGraph::build(vec![importer_file], resolutions, lint_targets)
    }

    fn options(json: serde_json::Value) -> PackageAccessRuleOptions {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn non_ts_import_falls_back_to_default_importability() {
        let opts = options(serde_json::json!({ "defaultImportability": "package" }));
        let file = importer(
            "/proj/src/other/consumer.ts",
            vec![entry(
                EntryKind::ImportDefault,
                "default",
                "../button/Button.module.css",
                0,
            )],
        );
        let g = graph(
            file,
            &[(
                "../button/Button.module.css",
                "/proj/src/button/Button.module.css",
            )],
        );

        let diagnostics = check_graph(&g, &opts, Path::new("/proj"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message_id, MessageId::Package);
        assert_eq!(diagnostics[0].identifier, "default");
    }

    #[test]
    fn non_ts_import_from_inside_the_package_passes() {
        let opts = options(serde_json::json!({ "defaultImportability": "package" }));
        let file = importer(
            "/proj/src/button/Button.tsx",
            vec![entry(
                EntryKind::ImportDefault,
                "default",
                "./Button.module.css",
                0,
            )],
        );
        let g = graph(
            file,
            &[("./Button.module.css", "/proj/src/button/Button.module.css")],
        );

        assert!(check_graph(&g, &opts, Path::new("/proj")).is_empty());
    }

    #[test]
    fn non_ts_exact_name_beats_star_and_star_never_matches_default() {
        // default -> public (exact), named -> package (via `*`), and a file whose
        // mapping has only `*` leaves `default` to fall back to
        // defaultImportability (private here, to tell the fallback apart).
        let opts = options(serde_json::json!({
            "defaultImportability": "private",
            "nonTsFiles": {
                "**/*.module.css": { "default": "public", "*": "package" },
                "**/*.json": { "*": "public" }
            }
        }));
        let file = importer(
            "/proj/src/other/consumer.ts",
            vec![
                entry(EntryKind::ImportDefault, "default", "../ui/a.module.css", 0),
                entry(EntryKind::Import, "button", "../ui/a.module.css", 10),
                entry(EntryKind::ImportDefault, "default", "../ui/data.json", 20),
                entry(EntryKind::Import, "field", "../ui/data.json", 30),
            ],
        );
        let g = graph(
            file,
            &[
                ("../ui/a.module.css", "/proj/src/ui/a.module.css"),
                ("../ui/data.json", "/proj/src/ui/data.json"),
            ],
        );

        let diagnostics = check_graph(&g, &opts, Path::new("/proj"));
        // css default: public -> pass. css named: package, cross-package -> fail.
        // json default: `*` skips it -> defaultImportability private -> fail.
        // json named: `*` public -> pass.
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].identifier, "button");
        assert_eq!(diagnostics[0].message_id, MessageId::Package);
        assert_eq!(diagnostics[1].identifier, "default");
        assert_eq!(diagnostics[1].message_id, MessageId::Private);
    }

    #[test]
    fn non_ts_first_matching_entry_wins() {
        let specific_first = options(serde_json::json!({
            "nonTsFiles": {
                "**/global.css": { "*": "public" },
                "**/*.css": { "*": "package" }
            }
        }));
        let general_first = options(serde_json::json!({
            "nonTsFiles": {
                "**/*.css": { "*": "package" },
                "**/global.css": { "*": "public" }
            }
        }));
        let file = importer(
            "/proj/src/other/consumer.ts",
            vec![entry(EntryKind::Import, "x", "../styles/global.css", 0)],
        );
        let g = graph(
            file,
            &[("../styles/global.css", "/proj/src/styles/global.css")],
        );

        // Specific entry first: `public` wins, no violation.
        assert!(check_graph(&g, &specific_first, Path::new("/proj")).is_empty());
        // General entry first: its `package` wins and the cross-package import fails.
        assert_eq!(check_graph(&g, &general_first, Path::new("/proj")).len(), 1);
    }

    #[test]
    fn non_ts_private_reexport_gets_the_reexport_message() {
        let opts = options(serde_json::json!({
            "nonTsFiles": { "**/*.css": { "*": "private" } }
        }));
        let file = importer(
            "/proj/src/button/index.ts",
            vec![entry(EntryKind::ReExport, "button", "./a.css", 0)],
        );
        let g = graph(file, &[("./a.css", "/proj/src/button/a.css")]);

        let diagnostics = check_graph(&g, &opts, Path::new("/proj"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message_id, MessageId::PrivateReexport);
    }

    #[test]
    fn exclude_source_patterns_applies_to_non_ts_exporters() {
        let opts = options(serde_json::json!({
            "defaultImportability": "package",
            "excludeSourcePatterns": ["**/*.css"]
        }));
        let file = importer(
            "/proj/src/other/consumer.ts",
            vec![entry(EntryKind::ImportDefault, "default", "../ui/a.css", 0)],
        );
        let g = graph(file, &[("../ui/a.css", "/proj/src/ui/a.css")]);

        assert!(check_graph(&g, &opts, Path::new("/proj")).is_empty());
    }
}
