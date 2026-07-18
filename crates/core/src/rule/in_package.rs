//! Port of the reference plugin's `isInPackage` (spec §4.2–§4.4): decides
//! whether an importer and a `package`-access exporter live in "the same package",
//! where "package" means a directory (by default) or a glob-matched ancestor
//! directory (`packageDirectory` option), with two configurable loopholes
//! (`indexLoophole`, `filenameLoophole`).
//!
//! Ported field-for-field from the TypeScript reference (see the M3 brief for the
//! exact source quoted) rather than re-derived from the spec prose — glob-matching
//! order and which of the original/index-adjusted exporter path is used where are
//! all easy to get subtly wrong from prose alone.
//!
//! One deliberate divergence from the reference: when `packageDirectory` is set
//! and a file has NO matching ancestor, the reference falls back to the file's
//! own parent directory — silently resurrecting directory-per-package semantics
//! for every not-yet-migrated file, which makes gradual adoption of a naming
//! convention like `["**/*.package"]` impossible. ImportLint instead falls back
//! to the project root, so all files outside every configured boundary share one
//! project-wide package (see `find_package_directory`).

use std::path::{Path, PathBuf};

use globset::{Glob, GlobBuilder, GlobMatcher};

/// Pre-compiled, per-`check_graph`-call state for `is_in_package`: the loophole
/// flags plus (if `packageDirectory` was configured) the compiled glob patterns and
/// the project directory patterns are matched relative to.
pub struct CompiledPackageOptions {
    pub index_loophole: bool,
    pub filename_loophole: bool,
    pub package_directory: Option<Vec<CompiledPattern>>,
    pub project_directory: PathBuf,
}

/// One compiled `packageDirectory` glob pattern: `negate` is `true` for a `!`-prefixed
/// pattern (`matcher` is compiled from the pattern with the `!` stripped).
pub struct CompiledPattern {
    negate: bool,
    matcher: GlobMatcher,
}

/// Compile the `packageDirectory` option's raw pattern strings once per
/// `check_graph` call. A pattern that fails to compile as a glob is treated as one
/// that never matches (logged to stderr) rather than panicking the whole run.
pub fn compile_package_directory_patterns(patterns: &[String]) -> Vec<CompiledPattern> {
    patterns
        .iter()
        .map(|pattern| {
            let (negate, inner) = match pattern.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, pattern.as_str()),
            };
            let matcher = GlobBuilder::new(inner)
                .literal_separator(true)
                .build()
                .map(|glob| glob.compile_matcher())
                .unwrap_or_else(|err| {
                    eprintln!(
                        "import-lint: invalid packageDirectory pattern '{pattern}': {err}, treating as never-matching"
                    );
                    // A glob that can never match anything: an empty alternation.
                    Glob::new("\0__import_lint_never_matches__\0")
                        .expect("literal pattern always compiles")
                        .compile_matcher()
                });
            CompiledPattern { negate, matcher }
        })
        .collect()
}

/// `path.relative(from, to)`, Node's algorithm over two absolute paths: split
/// into components, drop the common prefix, then `..` for each remaining
/// `from` component followed by the remaining `to` components, joined with
/// `/` (always `/`, whatever the platform separator — the result feeds glob
/// matchers compiled with `/` as the literal separator). Two equal paths
/// yield `""`. Built on `Path::components()` rather than string-splitting so
/// Windows separators (and mixed-separator paths) divide correctly.
pub(crate) fn node_relative(from: &Path, to: &Path) -> String {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    let common = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut parts: Vec<&str> = std::iter::repeat_n("..", from_components.len() - common).collect();
    parts.extend(
        to_components[common..]
            .iter()
            .map(|c| c.as_os_str().to_str().unwrap_or("")),
    );
    parts.join("/")
}

/// Regex-equivalent of `/\/index\.[cm]?[jt]sx?$/`: does `ext` (everything after the
/// last `/index.` in the path) fully match `[cm]?[jt]sx?`?
fn is_index_extension(ext: &str) -> bool {
    let mut rest = ext;
    if let Some(stripped) = rest.strip_prefix('c').or_else(|| rest.strip_prefix('m')) {
        rest = stripped;
    }
    let mut chars = rest.chars();
    match chars.next() {
        Some('j') | Some('t') => {}
        _ => return false,
    }
    if chars.next() != Some('s') {
        return false;
    }
    match chars.next() {
        None => true,
        Some('x') => chars.next().is_none(),
        _ => false,
    }
}

/// If `exporter`'s final path component is an index file (`index.js`, `index.tsx`,
/// `index.mjs`, ... but NOT `index.d.ts`), return its parent directory (the
/// index-loophole "stripped" path fed onward as the fake exporter "file" per the
/// reference implementation); otherwise return `exporter` unchanged.
fn strip_index_loophole(exporter: &Path) -> PathBuf {
    let stripped = exporter
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            let ext = name.strip_prefix("index.")?;
            is_index_extension(ext).then_some(())
        });
    if stripped.is_some() {
        exporter
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| exporter.to_path_buf())
    } else {
        exporter.to_path_buf()
    }
}

/// Does `dir` qualify as a "package directory" under `patterns`? A pattern is
/// tried against both `dir`'s basename and its path relative to
/// `project_directory`; a `!`-prefixed pattern that matches either immediately
/// disqualifies `dir` regardless of other patterns, a non-negated match sets the
/// (initially `false`) result to `true`.
fn is_package_directory(
    dir: &Path,
    patterns: &[CompiledPattern],
    project_directory: &Path,
) -> bool {
    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let relative_path = node_relative(project_directory, dir);

    let mut matched = false;
    for pattern in patterns {
        let is_match =
            pattern.matcher.is_match(dir_name) || pattern.matcher.is_match(&relative_path);
        if pattern.negate {
            if is_match {
                return false;
            }
        } else if is_match {
            matched = true;
        }
    }
    matched
}

/// Walk up from `file_path`'s parent directory to the filesystem root, returning
/// the first ancestor directory that qualifies as a package directory under
/// `patterns`; falls back to `project_directory` if none does, so every file
/// outside all configured boundaries belongs to a single project-root package.
/// (Deliberate divergence from the reference plugin, which falls back to the
/// file's own parent directory — see the module docs.)
fn find_package_directory(
    file_path: &Path,
    patterns: &[CompiledPattern],
    project_directory: &Path,
) -> PathBuf {
    let root = Path::new("/");
    let mut dir = file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| file_path.to_path_buf());

    while dir != root {
        if is_package_directory(&dir, patterns, project_directory) {
            return dir;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => break,
        }
    }
    project_directory.to_path_buf()
}

/// `getPackageDirectory`: the "package directory" a file belongs to — either the
/// nearest glob-matched ancestor (if `packageDirectory` is configured and
/// non-empty), or simply the file's own containing directory.
fn get_package_directory(file_path: &Path, opts: &CompiledPackageOptions) -> PathBuf {
    match &opts.package_directory {
        Some(patterns) if !patterns.is_empty() => {
            find_package_directory(file_path, patterns, &opts.project_directory)
        }
        _ => file_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| file_path.to_path_buf()),
    }
}

/// Is `exporter` importable from `importer` under `opts`, ignoring access level
/// entirely (this only answers the "same package" question the `package` access
/// level's directory check needs)?
pub fn is_in_package(importer: &Path, exporter: &Path, opts: &CompiledPackageOptions) -> bool {
    let index_adjusted_exporter = if opts.index_loophole {
        strip_index_loophole(exporter)
    } else {
        exporter.to_path_buf()
    };

    let importer_package_dir = get_package_directory(importer, opts);
    let exporter_package_dir = get_package_directory(&index_adjusted_exporter, opts);

    if importer_package_dir == exporter_package_dir {
        return true;
    }

    if opts.filename_loophole {
        // NOTE: the ORIGINAL (non-index-stripped) exporter path, per the reference.
        let importer_dir = importer.parent().unwrap_or(importer);
        let exporter_dir = exporter.parent().unwrap_or(exporter);
        let rel = node_relative(importer_dir, exporter_dir);
        let importer_stem = importer.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if rel == importer_stem {
            return true;
        }
    }

    let rel = node_relative(&exporter_package_dir, &importer_package_dir);
    !rel.is_empty() && !rel.starts_with("..") && !rel.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> CompiledPackageOptions {
        CompiledPackageOptions {
            index_loophole: true,
            filename_loophole: false,
            package_directory: None,
            project_directory: PathBuf::from("/proj"),
        }
    }

    fn opts_with(
        index_loophole: bool,
        filename_loophole: bool,
        package_directory: Option<&[&str]>,
    ) -> CompiledPackageOptions {
        CompiledPackageOptions {
            index_loophole,
            filename_loophole,
            package_directory: package_directory.map(|patterns| {
                compile_package_directory_patterns(
                    &patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                )
            }),
            project_directory: PathBuf::from("/proj"),
        }
    }

    #[test]
    fn same_directory_passes() {
        let o = opts();
        assert!(is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/b.ts"),
            &o
        ));
    }

    #[test]
    fn sibling_directories_fail() {
        let o = opts();
        assert!(!is_in_package(
            Path::new("/proj/src/sub1/a.ts"),
            Path::new("/proj/src/sub2/b.ts"),
            &o
        ));
    }

    #[test]
    fn importer_descendant_of_exporter_package_passes() {
        let o = opts();
        assert!(is_in_package(
            Path::new("/proj/src/sub/nested/a.ts"),
            Path::new("/proj/src/b.ts"),
            &o
        ));
    }

    #[test]
    fn importer_ancestor_of_exporter_package_fails() {
        let o = opts();
        assert!(!is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/sub/b.ts"),
            &o
        ));
    }

    #[test]
    fn index_loophole_allows_sibling_of_index_directory() {
        let o = opts_with(true, false, None);
        assert!(is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/sub/index.ts"),
            &o
        ));
    }

    #[test]
    fn index_loophole_disabled_sibling_of_index_directory_fails() {
        let o = opts_with(false, false, None);
        assert!(!is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/sub/index.ts"),
            &o
        ));
    }

    #[test]
    fn index_d_ts_is_not_stripped() {
        let o = opts_with(true, false, None);
        // index.d.ts must NOT trigger the loophole: sibling of "sub" fails, same as
        // if indexLoophole were false.
        assert!(!is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/sub/index.d.ts"),
            &o
        ));
    }

    #[test]
    fn filename_loophole_exact_one_level_passes() {
        let o = opts_with(true, true, None);
        // importer "foo.ts" importing from "foo/bar.ts" (one level down, named
        // after the importer's own stem).
        assert!(is_in_package(
            Path::new("/proj/src/foo.ts"),
            Path::new("/proj/src/foo/bar.ts"),
            &o
        ));
    }

    #[test]
    fn filename_loophole_two_levels_fails() {
        let o = opts_with(true, true, None);
        assert!(!is_in_package(
            Path::new("/proj/src/foo.ts"),
            Path::new("/proj/src/foo/nested/bar.ts"),
            &o
        ));
    }

    #[test]
    fn filename_loophole_wrong_directory_name_fails() {
        let o = opts_with(true, true, None);
        assert!(!is_in_package(
            Path::new("/proj/src/foo.ts"),
            Path::new("/proj/src/notfoo/bar.ts"),
            &o
        ));
    }

    #[test]
    fn package_directory_double_star_matches_default_behavior() {
        let o = opts_with(true, false, Some(&["**"]));
        assert!(is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/b.ts"),
            &o
        ));
        assert!(!is_in_package(
            Path::new("/proj/src/sub1/a.ts"),
            Path::new("/proj/src/sub2/b.ts"),
            &o
        ));
    }

    #[test]
    fn package_directory_negation_skips_internal_up_to_parent() {
        // packageDirectory: ["**", "!**/_internal"], exporter sits in x/_internal —
        // the package dir walk for the exporter skips "_internal" (negated) and
        // lands on "x", so an importer directly in "x" is considered same-package.
        let o = opts_with(true, false, Some(&["**", "!**/_internal"]));
        assert!(is_in_package(
            Path::new("/proj/x/user.ts"),
            Path::new("/proj/x/_internal/helper.ts"),
            &o
        ));
    }

    #[test]
    fn package_directory_unmatched_files_share_root_package() {
        // Neither file has a *.package ancestor, so both fall back to the
        // project root as their package — even across unrelated directories.
        // (Under the reference plugin's parent-directory fallback this failed,
        // which made gradual adoption of the naming convention impossible.)
        let o = opts_with(true, false, Some(&["*.package"]));
        assert!(is_in_package(
            Path::new("/proj/src/a/x.ts"),
            Path::new("/proj/lib/b/y.ts"),
            &o
        ));
    }

    #[test]
    fn package_directory_unmatched_importer_cannot_reach_into_package() {
        // The importer falls back to the root package; the exporter is inside
        // auth.package. Root is an ancestor of the boundary, not a descendant,
        // so the import fails — encapsulation of matched boundaries still holds.
        let o = opts_with(true, false, Some(&["*.package"]));
        assert!(!is_in_package(
            Path::new("/proj/src/a.ts"),
            Path::new("/proj/src/auth.package/b.ts"),
            &o
        ));
    }

    #[test]
    fn package_directory_root_package_export_reachable_from_inside_package() {
        // The exporter falls back to the root package; an importer inside
        // auth.package is a descendant of the root, so the import is allowed
        // (same ancestor-package rule as nested matched boundaries).
        let o = opts_with(true, false, Some(&["*.package"]));
        assert!(is_in_package(
            Path::new("/proj/src/auth.package/a.ts"),
            Path::new("/proj/src/b.ts"),
            &o
        ));
    }

    #[test]
    fn package_directory_relative_path_pattern() {
        let o = opts_with(true, false, Some(&["src/a/b/*"]));
        // Both files' package directory resolves to /proj/src/a/b/c via the
        // project-relative pattern match (not a basename match — "c" alone does
        // not match the literal pattern "src/a/b/*").
        assert!(is_in_package(
            Path::new("/proj/src/a/b/c/user.ts"),
            Path::new("/proj/src/a/b/c/helper.ts"),
            &o
        ));
        // A file nested one level deeper still resolves to the same package
        // directory: the walk-up passes through "nested" (no match) up to "c"
        // (matches).
        assert!(is_in_package(
            Path::new("/proj/src/a/b/c/nested/user.ts"),
            Path::new("/proj/src/a/b/c/helper.ts"),
            &o
        ));
        // Sibling package "d" is a different package directory.
        assert!(!is_in_package(
            Path::new("/proj/src/a/b/c/user.ts"),
            Path::new("/proj/src/a/b/d/helper.ts"),
            &o
        ));
    }
}
