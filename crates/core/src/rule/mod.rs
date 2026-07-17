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
pub use options::{Importability, PackageAccessRuleOptions, SelfRefOpt};

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

            let Some((exporter_path, info, identifier)) = lookup(graph, target, entry) else {
                continue;
            };

            if !exclude_patterns.is_empty() {
                let relative = in_package::node_relative(project_root, &exporter_path);
                if exclude_patterns.iter().any(|m| m.is_match(&relative)) {
                    continue;
                }
            }

            let access = info.access.unwrap_or(default_access);
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
