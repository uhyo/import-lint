//! In-memory overlay content for open editor buffers (L1, `docs/PLAN.md` §3): an
//! overlay's content overrides its file's on-disk content in extraction (`runner.rs`)
//! and in diagnostic line/col rendering (`report.rs`), while everything else about the
//! pipeline — walking, resolution, the check phase — is unaffected. [`Overlays`] is
//! owned by `watch::WatchSession` and empty by default, which is what the one-shot CLI
//! path (`runner::run`) and non-LSP watch mode always pass, so behavior is unchanged
//! unless something actually populates it (the future `import-lint lsp` server, L2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One overlay's content plus the version it was set at.
#[derive(Debug)]
struct OverlayEntry {
    content: Arc<str>,
    version: u64,
}

/// In-memory overlay store, keyed by the same path identity used everywhere else in
/// the module graph (a canonical path — see `WatchSession::set_overlay`'s doc
/// comment). Every [`Overlays::set`] call bumps a store-wide monotonic version
/// counter: `runner.rs`'s `SourceStamp::Overlay` compares an entry's version rather
/// than hashing content, so re-setting a buffer to the *same* text still counts as a
/// change (matches the disk-mtime path's own good-enough-not-exact semantics) and
/// invalidates any `ExtractionCache` entry keyed to the previous version.
#[derive(Default, Debug)]
pub struct Overlays {
    entries: HashMap<PathBuf, OverlayEntry>,
    next_version: u64,
}

impl Overlays {
    /// Set (or replace) the overlay for `path`, assigning it a new version strictly
    /// greater than any previously assigned by this store.
    pub fn set(&mut self, path: PathBuf, content: String) {
        self.next_version += 1;
        self.entries.insert(
            path,
            OverlayEntry {
                content: Arc::from(content),
                version: self.next_version,
            },
        );
    }

    /// Remove the overlay for `path`, if any. Returns whether one existed.
    pub fn clear(&mut self, path: &Path) -> bool {
        self.entries.remove(path).is_some()
    }

    /// Whether no overlays are currently set.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `path`'s overlay content, if any — cheap to clone (an `Arc<str>` handle, not a
    /// copy of the text).
    pub(crate) fn content(&self, path: &Path) -> Option<Arc<str>> {
        self.entries.get(path).map(|entry| entry.content.clone())
    }

    /// `path`'s overlay version, if any (`runner.rs`'s `SourceStamp::Overlay`).
    pub(crate) fn version(&self, path: &Path) -> Option<u64> {
        self.entries.get(path).map(|entry| entry.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_assigns_a_strictly_increasing_version_on_each_call() {
        let mut overlays = Overlays::default();
        let path = PathBuf::from("/proj/a.ts");

        overlays.set(path.clone(), "a".to_string());
        let v1 = overlays.version(&path).expect("overlay set");

        overlays.set(path.clone(), "a".to_string());
        let v2 = overlays.version(&path).expect("overlay set");

        assert!(
            v2 > v1,
            "re-setting to identical content must still bump the version"
        );
    }

    #[test]
    fn version_counter_is_shared_across_paths() {
        let mut overlays = Overlays::default();
        overlays.set(PathBuf::from("/proj/a.ts"), "a".to_string());
        overlays.set(PathBuf::from("/proj/b.ts"), "b".to_string());

        let va = overlays.version(&PathBuf::from("/proj/a.ts")).unwrap();
        let vb = overlays.version(&PathBuf::from("/proj/b.ts")).unwrap();
        assert!(vb > va);
    }

    #[test]
    fn clear_reports_whether_an_overlay_existed() {
        let mut overlays = Overlays::default();
        let path = PathBuf::from("/proj/a.ts");

        assert!(!overlays.clear(&path), "nothing to clear yet");

        overlays.set(path.clone(), "a".to_string());
        assert!(overlays.clear(&path), "an overlay was set");
        assert!(!overlays.clear(&path), "already cleared");
    }

    #[test]
    fn content_reflects_the_most_recent_set() {
        let mut overlays = Overlays::default();
        let path = PathBuf::from("/proj/a.ts");

        assert!(overlays.content(&path).is_none());

        overlays.set(path.clone(), "first".to_string());
        assert_eq!(overlays.content(&path).as_deref(), Some("first"));

        overlays.set(path.clone(), "second".to_string());
        assert_eq!(overlays.content(&path).as_deref(), Some("second"));
    }

    #[test]
    fn is_empty_tracks_entries() {
        let mut overlays = Overlays::default();
        assert!(overlays.is_empty());

        let path = PathBuf::from("/proj/a.ts");
        overlays.set(path.clone(), "a".to_string());
        assert!(!overlays.is_empty());

        overlays.clear(&path);
        assert!(overlays.is_empty());
    }
}
