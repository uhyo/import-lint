//! File discovery (PLAN.md §2.1 step 2, M2): the `ignore` crate's parallel walker,
//! rooted at the configured include paths.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};

/// Extensions ImportLint parses (`.d.ts`/`.d.mts`/`.d.cts` included automatically —
/// their final extension is `.ts`/`.mts`/`.cts`).
const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Discover every source file reachable from `roots`, with no extra exclude globs
/// beyond `.gitignore`. See [`walk_with_excludes`] for the full behavior; this is a
/// thin wrapper kept for callers (and tests) that don't need config `exclude`.
pub fn walk(roots: &[PathBuf]) -> Vec<PathBuf> {
    walk_with_excludes(roots, None, &[])
}

/// Discover every source file reachable from `roots`. Respects `.gitignore` (the
/// `ignore` crate's default); `node_modules` directories are additionally always
/// skipped regardless of ignore rules (belt and braces — most projects gitignore it,
/// but this must hold even for a project that doesn't). A root that doesn't exist is
/// skipped with a warning to stderr rather than panicking.
///
/// `exclude` is a list of extra glob patterns (config `exclude`, M5) applied on top
/// of `.gitignore`, rooted at `project_root` (falls back to the first existing root
/// when `project_root` is `None`, matching `walk`'s no-config behavior). Each
/// pattern is added as a gitignore-style ignore glob (`OverrideBuilder`'s `!pattern`
/// = exclude matching paths — PLAN.md M5 brief).
///
/// Returned paths are canonicalized (absolute, symlinks resolved) and returned sorted
/// with duplicates removed, so the pipeline's output is deterministic across runs and
/// across overlapping roots.
pub fn walk_with_excludes(
    roots: &[PathBuf],
    project_root: Option<&Path>,
    exclude: &[String],
) -> Vec<PathBuf> {
    // Strip `.` (`Component::CurDir`) segments before handing paths to `ignore`:
    // a root like `<project_root>/.` (produced whenever a config's `include`
    // defaults to `["."]`, joined via `project_root.join(".")`) makes
    // `OverrideBuilder`'s anchored-glob relative-path computation land one
    // component off, so an exclude pattern like `src/a.ts` silently fails to
    // match a real `src/a.ts` file. Trailing/embedded `.` segments are otherwise
    // inert (the filesystem itself doesn't care), so this is a pure no-op fix.
    let roots: Vec<PathBuf> = roots.iter().map(|root| strip_cur_dir(root)).collect();
    let project_root = project_root.map(strip_cur_dir);

    let mut existing_roots = roots.iter();
    let Some(first_root) = existing_roots.find(|root| check_root_exists(root)) else {
        return Vec::new();
    };

    let mut builder = WalkBuilder::new(first_root);
    for root in existing_roots {
        if check_root_exists(root) {
            builder.add(root);
        }
    }
    // Respect `.gitignore` even when the tree being linted isn't inside an actual
    // git checkout (e.g. a CI artifact or extracted tarball) — the `ignore` crate's
    // default otherwise silently ignores `.gitignore` files outside a `.git` repo.
    builder.require_git(false);

    if !exclude.is_empty() {
        let override_root = project_root.as_deref().unwrap_or(first_root);
        let mut override_builder = OverrideBuilder::new(override_root);
        for pattern in exclude {
            let ignore_glob = format!("!{pattern}");
            if let Err(err) = override_builder.add(&ignore_glob) {
                eprintln!("import-lint: invalid exclude pattern '{pattern}': {err}, ignoring");
            }
        }
        match override_builder.build() {
            Ok(overrides) => {
                builder.overrides(overrides);
            }
            Err(err) => {
                eprintln!(
                    "import-lint: failed to build exclude overrides: {err}, ignoring excludes"
                );
            }
        }
    }

    let found: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    eprintln!("import-lint: {err}, skipping");
                    return WalkState::Continue;
                }
            };

            // Belt and braces: never descend into `node_modules`, even if some
            // project's ignore rules don't already cover it.
            if entry.file_name() == "node_modules" {
                return WalkState::Skip;
            }

            let is_file = entry.file_type().is_some_and(|ft| ft.is_file());
            if is_file && has_source_extension(entry.path()) {
                match entry.path().canonicalize() {
                    Ok(canonical) => found.lock().unwrap().push(canonical),
                    Err(err) => {
                        eprintln!("import-lint: {}: {err}, skipping", entry.path().display());
                    }
                }
            }
            WalkState::Continue
        })
    });

    let mut files = found.into_inner().unwrap();
    files.sort();
    files.dedup();
    files
}

/// Remove every `.` (`Component::CurDir`) segment from `path`, leaving everything
/// else (including `..` segments) untouched. See the comment at
/// [`walk_with_excludes`]'s call site for why this matters.
fn strip_cur_dir(path: &Path) -> PathBuf {
    use std::path::Component;

    let cleaned: PathBuf = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

fn check_root_exists(root: &Path) -> bool {
    if root.exists() {
        true
    } else {
        eprintln!(
            "import-lint: {}: no such file or directory, skipping",
            root.display()
        );
        false
    }
}

fn has_source_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, relative: &str, contents: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
    }

    /// Regression test: a walk root with a literal `.` segment (exactly what
    /// `project_root.join(".")` produces for a config's default `include: ["."]`)
    /// must not break anchored exclude-glob matching.
    #[test]
    fn exclude_glob_matches_through_a_dotted_root() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "src/a.ts", "export const a = 1;\n");
        write(dir.path(), "src/b.ts", "export const b = 1;\n");

        let found = walk_with_excludes(
            &[dir.path().join(".")],
            Some(dir.path()),
            &["src/a.ts".to_string()],
        );

        assert!(!found.iter().any(|p| p.ends_with("a.ts")));
        assert!(found.iter().any(|p| p.ends_with("b.ts")));
    }

    #[test]
    fn exclude_glob_matches_with_a_clean_root() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "src/a.ts", "export const a = 1;\n");
        write(dir.path(), "src/b.ts", "export const b = 1;\n");

        let found = walk_with_excludes(
            &[dir.path().to_path_buf()],
            Some(dir.path()),
            &["src/a.ts".to_string()],
        );

        assert!(!found.iter().any(|p| p.ends_with("a.ts")));
        assert!(found.iter().any(|p| p.ends_with("b.ts")));
    }

    #[test]
    fn strip_cur_dir_removes_only_dot_segments() {
        assert_eq!(strip_cur_dir(Path::new("/a/./b")), PathBuf::from("/a/b"));
        assert_eq!(strip_cur_dir(Path::new("/a/b/.")), PathBuf::from("/a/b"));
        assert_eq!(strip_cur_dir(Path::new("../a")), PathBuf::from("../a"));
        assert_eq!(strip_cur_dir(Path::new(".")), PathBuf::from("."));
    }
}
