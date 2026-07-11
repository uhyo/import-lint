//! The whole-project module graph assembled after the link phase (PLAN-v1.md §2.1 step
//! 4, §2.2, M2).
//!
//! `build` is a pure, single-threaded assembly step: the CLI crate parallelizes
//! extraction and resolution (rayon) and hands the results here once per run (or
//! per watch re-link, PLAN-v1.md §7) — no rayon in core.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_str::CompactStr;

use crate::extract::FileModuleInfo;
use crate::resolve::Provenance;

/// Every extracted file, every computed resolution, and the reverse edges derived
/// from them.
pub struct ModuleGraph {
    /// Every extracted file: files discovered by the walker plus any file only
    /// reached as an internal resolution target (it still needs to be in here as
    /// an export-table lookup target for the one-hop check).
    pub files: HashMap<PathBuf, Arc<FileModuleInfo>>,
    /// `(importer file, specifier)` -> how that specifier resolved.
    pub resolutions: HashMap<(PathBuf, CompactStr), Provenance>,
    /// Reverse index: target file -> files with *any* edge into it (import,
    /// default import, re-export, or star export specifier that resolved to it).
    /// Watch mode's dirty-set computation (PLAN-v1.md §7) starts here.
    pub importers: HashMap<PathBuf, HashSet<PathBuf>>,
    /// Reverse index restricted to `export * from` edges: target file -> files
    /// that star-export it. A subset of `importers`, kept separate because watch
    /// mode needs to know specifically which files might need to re-descend a
    /// star-export chain when a file's export surface changes (PLAN-v1.md §7).
    pub star_importers: HashMap<PathBuf, HashSet<PathBuf>>,
    /// Files discovered by the walker (as opposed to files only present because
    /// they're a resolution target) — only these files' `checked_entries` get
    /// linted.
    pub lint_targets: HashSet<PathBuf>,
}

impl ModuleGraph {
    /// Assemble the graph. `resolutions` must already contain an entry for every
    /// `(file.path, specifier)` pair reachable from `files`' `specifiers` lists
    /// (the link phase's job); both reverse indexes are derived purely from that
    /// map plus each file's `star_exports`.
    pub fn build(
        files: Vec<Arc<FileModuleInfo>>,
        resolutions: HashMap<(PathBuf, CompactStr), Provenance>,
        lint_targets: HashSet<PathBuf>,
    ) -> Self {
        // Every `(file, specifier)` pair that's a star-export edge, so the single
        // pass over `resolutions` below can tell star edges apart from ordinary
        // ones without doing a second keyed lookup per file.
        let star_edges: HashSet<(PathBuf, CompactStr)> = files
            .iter()
            .flat_map(|file| {
                file.star_exports
                    .iter()
                    .map(move |specifier| (file.path.clone(), specifier.clone()))
            })
            .collect();

        let mut importers: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        let mut star_importers: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for (key, provenance) in &resolutions {
            let Provenance::Internal(target) = provenance else {
                continue;
            };
            let (importer, _) = key;
            importers
                .entry(target.clone())
                .or_default()
                .insert(importer.clone());
            if star_edges.contains(key) {
                star_importers
                    .entry(target.clone())
                    .or_default()
                    .insert(importer.clone());
            }
        }

        let files = files.into_iter().map(|f| (f.path.clone(), f)).collect();

        Self {
            files,
            resolutions,
            importers,
            star_importers,
            lint_targets,
        }
    }

    /// Look up how `specifier` resolved from `importer`. Allocates to build the
    /// lookup key (tuple keys can't be queried componentwise); callers on a hot
    /// path over many entries for the same importer should iterate `resolutions`
    /// directly instead.
    pub fn resolution(&self, importer: &Path, specifier: &str) -> Option<&Provenance> {
        self.resolutions
            .get(&(importer.to_path_buf(), CompactStr::from(specifier)))
    }

    pub fn file(&self, path: &Path) -> Option<&Arc<FileModuleInfo>> {
        self.files.get(path)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn file(path: &str, specifiers: &[&str], star_exports: &[&str]) -> Arc<FileModuleInfo> {
        Arc::new(FileModuleInfo {
            path: PathBuf::from(path),
            checked_entries: Vec::new(),
            export_table: HashMap::new(),
            star_exports: star_exports.iter().map(|s| CompactStr::from(*s)).collect(),
            ambient_modules: Vec::new(),
            specifiers: specifiers.iter().map(|s| CompactStr::from(*s)).collect(),
        })
    }

    #[test]
    fn reverse_indexes_and_star_importer_separation() {
        // a.ts imports "./b" (an ordinary import) and star-exports "./c"; both
        // resolve to internal files. importers should record both edges; only the
        // star-export edge should show up in star_importers.
        let a = file("/proj/a.ts", &["./b", "./c"], &["./c"]);
        let b = file("/proj/b.ts", &[], &[]);
        let c = file("/proj/c.ts", &[], &[]);

        let mut resolutions = HashMap::new();
        resolutions.insert(
            (a.path.clone(), CompactStr::from("./b")),
            Provenance::Internal(b.path.clone()),
        );
        resolutions.insert(
            (a.path.clone(), CompactStr::from("./c")),
            Provenance::Internal(c.path.clone()),
        );

        let lint_targets = HashSet::from([a.path.clone()]);
        let graph = ModuleGraph::build(
            vec![a.clone(), b.clone(), c.clone()],
            resolutions,
            lint_targets,
        );

        assert_eq!(
            graph.importers.get(&b.path).cloned().unwrap_or_default(),
            HashSet::from([a.path.clone()])
        );
        assert_eq!(
            graph.importers.get(&c.path).cloned().unwrap_or_default(),
            HashSet::from([a.path.clone()])
        );
        // b is imported ordinarily, never star-exported: absent from star_importers.
        assert!(!graph.star_importers.contains_key(&b.path));
        // c is star-exported: present, and only via the star edge.
        assert_eq!(
            graph
                .star_importers
                .get(&c.path)
                .cloned()
                .unwrap_or_default(),
            HashSet::from([a.path.clone()])
        );

        assert_eq!(
            graph.resolution(&a.path, "./b"),
            Some(&Provenance::Internal(b.path.clone()))
        );
        assert_eq!(graph.resolution(&a.path, "./nope"), None);
        assert!(graph.file(&a.path).is_some());
        assert!(graph.file(Path::new("/proj/does-not-exist.ts")).is_none());
        assert!(graph.lint_targets.contains(&a.path));
        assert!(!graph.lint_targets.contains(&b.path));
    }

    #[test]
    fn external_and_unresolved_resolutions_do_not_create_reverse_edges() {
        let a = file("/proj/a.ts", &["pkg", "./missing"], &[]);

        let mut resolutions = HashMap::new();
        resolutions.insert(
            (a.path.clone(), CompactStr::from("pkg")),
            Provenance::External,
        );
        resolutions.insert(
            (a.path.clone(), CompactStr::from("./missing")),
            Provenance::Unresolved,
        );

        let graph = ModuleGraph::build(vec![a.clone()], resolutions, HashSet::new());

        assert!(graph.importers.is_empty());
        assert!(graph.star_importers.is_empty());
    }
}
