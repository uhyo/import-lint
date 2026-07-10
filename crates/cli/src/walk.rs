//! File discovery (PLAN.md §2.1 step 2, M2): the `ignore` crate's parallel walker,
//! rooted at the configured include paths.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::{WalkBuilder, WalkState};

/// Extensions ImportLint parses (`.d.ts`/`.d.mts`/`.d.cts` included automatically —
/// their final extension is `.ts`/`.mts`/`.cts`).
const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Discover every source file reachable from `roots`. Respects `.gitignore` (the
/// `ignore` crate's default); `node_modules` directories are additionally always
/// skipped regardless of ignore rules (belt and braces — most projects gitignore it,
/// but this must hold even for a project that doesn't). A root that doesn't exist is
/// skipped with a warning to stderr rather than panicking.
///
/// Returned paths are canonicalized (absolute, symlinks resolved) and returned sorted
/// with duplicates removed, so the pipeline's output is deterministic across runs and
/// across overlapping roots.
pub fn walk(roots: &[PathBuf]) -> Vec<PathBuf> {
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
