//! Watch mode tests (PLAN.md §7, M6). Two kinds:
//!
//! - Session-level tests drive `WatchSession::run_cycle` with synthesized
//!   `ChangeKind` batches directly — no `notify` involved (M6 brief D-W3), so these
//!   are fast and deterministic.
//! - One real-watcher smoke test drives `watch_loop` with an actual `PollWatcher`
//!   against a real edit on disk, to prove the `notify` integration itself works
//!   end-to-end (not just the pure classification/session logic covered elsewhere).
//!
//! The notify-event -> `ChangeKind` classification function (`classify_event`) is
//! pure and unit-tested alongside its definition in `crates/cli/src/watch.rs`
//! (`watch::classify_tests`), not here.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use import_lint_cli::output::OutputSeverity;
use import_lint_cli::watch::{ChangeKind, WatchSession, WatchSessionOptions, watch_loop};
use tempfile::TempDir;

fn write(dir: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|err| panic!("canonicalize {}: {err}", path.display()))
}

/// Default watch-session options: lint `dir` itself (no `--config`/`--tsconfig`
/// override, no `--report-unresolved`/`--quiet`) — config discovery still applies,
/// so a `.importlintrc.jsonc` written into `dir` before `WatchSession::new` is
/// picked up, matching non-watch `lint()`'s behavior.
fn session_options(dir: &Path) -> WatchSessionOptions {
    WatchSessionOptions {
        cli_paths: Vec::new(),
        explicit_config: None,
        cli_tsconfig: None,
        report_unresolved: false,
        quiet: false,
        cwd: dir.to_path_buf(),
    }
}

/// A single `@package` violation: `src/consumer.ts` imports `helper` from
/// `src/internal/util.ts`, which the default options forbid (sibling directories,
/// not ancestor/descendant — mirrors `crates/cli/tests/cli.rs`'s fixture).
fn write_violation_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let consumer = write(
        dir,
        "src/consumer.ts",
        "import { helper } from \"./internal/util\";\nconsole.log(helper);\n",
    );
    let util = write(
        dir,
        "src/internal/util.ts",
        "/** @package */\nexport const helper = 1;\n",
    );
    (canonical(&consumer), canonical(&util))
}

#[test]
fn content_edit_removing_the_import_clears_the_violation() {
    let dir = TempDir::new().unwrap();
    let (consumer, _util) = write_violation_fixture(dir.path());

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(session.last_diagnostics().len(), 1);
    assert!(session.last_has_error());

    fs::write(&consumer, "console.log(1);\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(consumer)]);

    assert!(outcome.diagnostics.is_empty());
    assert!(!outcome.has_error);
}

#[test]
fn content_edit_of_exporters_jsdoc_propagates_through_the_full_recheck() {
    let dir = TempDir::new().unwrap();
    let (_consumer, util) = write_violation_fixture(dir.path());

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(session.last_diagnostics().len(), 1);

    // @package -> @public: the violation disappears.
    fs::write(&util, "/** @public */\nexport const helper = 1;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(util.clone())]);
    assert!(
        outcome.diagnostics.is_empty(),
        "expected no diagnostics, got {:?}",
        outcome.diagnostics
    );

    // @public -> @package: the violation reappears.
    fs::write(&util, "/** @package */\nexport const helper = 1;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(util)]);
    assert_eq!(outcome.diagnostics.len(), 1);
}

#[test]
fn structural_change_detects_a_newly_added_violating_file() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "src/internal/util.ts",
        "/** @package */\nexport const helper = 1;\n",
    );

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert!(session.last_diagnostics().is_empty());

    write(
        dir.path(),
        "src/new_consumer.ts",
        "import { helper } from \"./internal/util\";\nconsole.log(helper);\n",
    );
    let outcome = session.run_cycle(&[ChangeKind::Structural]);

    assert_eq!(outcome.diagnostics.len(), 1);
    assert!(outcome.diagnostics[0].file.ends_with("new_consumer.ts"));
}

#[test]
fn structural_change_detects_a_deleted_exporter_and_drops_its_diagnostic() {
    let dir = TempDir::new().unwrap();
    let (_consumer, util) = write_violation_fixture(dir.path());

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(session.last_diagnostics().len(), 1);

    fs::remove_file(&util).unwrap();
    let outcome = session.run_cycle(&[ChangeKind::Structural]);

    // The import now fails to resolve (D8: unresolved imports are skipped, not
    // reported) rather than producing a stale/dangling diagnostic.
    assert!(outcome.diagnostics.is_empty());
}

#[test]
fn config_changed_reloads_severity_and_survives_a_subsequently_invalid_config() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    let config_path = write(dir.path(), ".importlintrc.jsonc", "{}");

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(
        session.last_diagnostics()[0].severity,
        OutputSeverity::Error
    );

    fs::write(
        &config_path,
        r#"{ "rules": { "jsdoc": { "severity": "warn" } } }"#,
    )
    .unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ConfigChanged]);
    assert!(outcome.config_error.is_none());
    assert_eq!(outcome.diagnostics.len(), 1);
    assert_eq!(outcome.diagnostics[0].severity, OutputSeverity::Warn);
    assert!(!outcome.has_error);

    // An invalid edit: the previous (warn) config is kept, not a crash or an
    // unexpected severity flip.
    fs::write(&config_path, "{ not valid json ").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ConfigChanged]);
    assert!(outcome.config_error.is_some());
    assert_eq!(outcome.diagnostics.len(), 1);
    assert_eq!(outcome.diagnostics[0].severity, OutputSeverity::Warn);
}

#[test]
fn cycle_with_no_changes_re_extracts_nothing() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");
    write(
        dir.path(),
        "src/b.ts",
        "import { a } from \"./a\";\nconsole.log(a);\n",
    );

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");

    let outcome = session.run_cycle(&[]);
    assert_eq!(outcome.extracted_files, 0);

    // An unrelated `Structural` batch also touches nothing on disk, so the
    // extraction cache should still serve every file from cache (re-walking and
    // rebuilding the resolver doesn't force re-parsing unchanged files).
    let outcome = session.run_cycle(&[ChangeKind::Structural]);
    assert_eq!(outcome.extracted_files, 0);
}

#[test]
fn content_edit_cycle_only_re_extracts_the_changed_file() {
    let dir = TempDir::new().unwrap();
    let a = canonical(&write(dir.path(), "src/a.ts", "export const a = 1;\n"));
    write(dir.path(), "src/b.ts", "export const b = 1;\n");

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");

    fs::write(&a, "export const a = 2;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(a)]);
    assert_eq!(outcome.extracted_files, 1);
}

#[test]
fn new_returns_an_error_for_a_missing_explicit_config_instead_of_starting() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");

    let options = WatchSessionOptions {
        cli_paths: Vec::new(),
        explicit_config: Some(dir.path().join("does-not-exist.jsonc")),
        cli_tsconfig: None,
        report_unresolved: false,
        quiet: false,
        cwd: dir.path().to_path_buf(),
    };

    let result = WatchSession::new(options);
    let Err(err) = result else {
        panic!("missing --config should be an error");
    };
    assert!(err.to_string().contains("does-not-exist.jsonc"));
}

/// The one real-watcher test (M6 brief §Tests item 2): drive `watch_loop` with a
/// real `PollWatcher` (used instead of the platform-recommended inotify watcher for
/// determinism in CI, per `docs/PLAN.md` §9.4) against an actual file edit, and
/// assert a `CycleOutcome` reflecting that edit arrives within a generous timeout.
/// This is the only thing in this file that isn't pure/deterministic by
/// construction, so it gets the long timeout and the most headroom.
#[test]
fn poll_watcher_smoke_test_detects_a_real_content_edit() {
    let dir = TempDir::new().unwrap();
    let (_consumer, util) = write_violation_fixture(dir.path());

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(session.last_diagnostics().len(), 1);

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_thread = shutdown.clone();
    let (tx, rx) = mpsc::channel::<(usize, bool)>();

    let handle = std::thread::spawn(move || {
        let poll_interval = Some(Duration::from_millis(100));
        let debounce = Duration::from_millis(150);
        let _ = watch_loop(
            &mut session,
            debounce,
            poll_interval,
            &shutdown_for_thread,
            |outcome| {
                let _ = tx.send((outcome.diagnostics.len(), outcome.has_error));
            },
        );
    });

    // Give the poll watcher time to complete its initial recursive baseline scan
    // before we edit — editing too soon gets silently absorbed into that baseline
    // scan instead of producing a diff (verified empirically: a 400ms margin here
    // was flaky, 2s was reliable across repeated runs).
    std::thread::sleep(Duration::from_millis(2000));
    fs::write(&util, "/** @public */\nexport const helper = 1;\n").unwrap();

    let received = rx.recv_timeout(Duration::from_secs(10));

    shutdown.store(true, Ordering::Relaxed);
    handle.join().expect("watch_loop thread should not panic");

    let (diagnostic_count, has_error) =
        received.expect("expected a watch cycle within 10s of editing the file");
    assert_eq!(diagnostic_count, 0, "expected the violation to clear");
    assert!(!has_error);
}
