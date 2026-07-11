//! Nearest-named-`package.json` lookup for self-reference detection (spec §4.6).
//!
//! `lookupPackageJson` in the reference plugin walks up from the *importer*'s
//! directory to the nearest `package.json` that has a `name` field (skipping over
//! any without one) and compares the specifier against that name, purely
//! string-based. This is unrelated to `oxc_resolver`'s own tsconfig/node_modules
//! package.json handling, so it's implemented as a small side cache here.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::Deserialize;

#[derive(Deserialize)]
struct PackageJsonName {
    name: Option<String>,
}

/// The nearest ancestor `package.json` (starting from some directory) that has a
/// non-empty `name` field.
#[derive(Debug, Clone)]
pub(super) struct PackageJsonEntry {
    pub name: String,
}

/// Caches, per starting directory, the result of walking up to the nearest named
/// `package.json` (or the absence of one). Thread-safe behind `&self` so a single
/// `ProjectResolver` can be shared across worker threads (PLAN-v1.md §8).
#[derive(Default)]
pub(super) struct PackageJsonCache {
    by_dir: RwLock<HashMap<PathBuf, Option<PackageJsonEntry>>>,
}

impl PackageJsonCache {
    /// `start_dir` is the importer file's containing directory.
    pub fn nearest_named(&self, start_dir: &Path) -> Option<PackageJsonEntry> {
        if let Some(cached) = self.by_dir.read().unwrap().get(start_dir) {
            return cached.clone();
        }
        let found = walk_up(start_dir);
        self.by_dir
            .write()
            .unwrap()
            .insert(start_dir.to_path_buf(), found.clone());
        found
    }
}

fn walk_up(start_dir: &Path) -> Option<PackageJsonEntry> {
    let mut dir = Some(start_dir);
    while let Some(d) = dir {
        let candidate = d.join("package.json");
        if let Ok(text) = fs::read_to_string(&candidate)
            && let Ok(parsed) = serde_json::from_str::<PackageJsonName>(&text)
            && let Some(name) = parsed.name.filter(|n| !n.is_empty())
        {
            return Some(PackageJsonEntry { name });
        }
        dir = d.parent();
    }
    None
}
