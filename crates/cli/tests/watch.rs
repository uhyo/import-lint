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

/// Fast-path star-export closure (M7, PLAN.md §7 "locked design" step 5):
/// `other/user.ts` imports `value` from `src/barrel.ts`, which only offers it via a
/// bare `export * from "./inner"` (no explicit re-export of its own, so `barrel.ts`
/// itself is never a checked entry) — the one-hop lookup for `user.ts` has to
/// descend through the star-export chain into `src/inner.ts`. Editing `inner.ts`'s
/// JSDoc access must propagate through that chain to `user.ts`'s diagnostic in both
/// directions, exercising `propagate_star_closure`'s recursive
/// `star_importers` walk, not just a direct-importer edge.
#[test]
fn content_edit_propagates_through_a_star_export_chain() {
    let dir = TempDir::new().unwrap();
    let inner = canonical(&write(
        dir.path(),
        "src/inner.ts",
        "/** @package */\nexport const value = 1;\n",
    ));
    write(dir.path(), "src/barrel.ts", "export * from \"./inner\";\n");
    write(
        dir.path(),
        "other/user.ts",
        "import { value } from \"../src/barrel\";\nconsole.log(value);\n",
    );

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(
        session.last_diagnostics().len(),
        1,
        "user.ts and inner.ts are in different directories, so @package should violate"
    );

    fs::write(&inner, "/** @public */\nexport const value = 1;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(inner.clone())]);
    assert!(
        outcome.diagnostics.is_empty(),
        "expected the star-chain violation to clear, got {:?}",
        outcome.diagnostics
    );

    fs::write(&inner, "/** @package */\nexport const value = 1;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(inner)]);
    assert_eq!(
        outcome.diagnostics.len(),
        1,
        "expected the star-chain violation to reappear, got {:?}",
        outcome.diagnostics
    );
}

/// The passthrough re-export one-hop rule under the fast path (M7): `src/sub/barrel.ts`
/// re-exports `x` from `src/inner.ts` via an *explicit* `/** @public */ export { x }
/// from "../inner"` — the re-export statement's own JSDoc, not `inner.ts`'s, governs
/// what `other/user.ts` (which imports `x` from the barrel) sees, per the one-hop
/// rule ("never hop a second time" — `crates/core/src/rule/mod.rs`). Editing
/// `inner.ts`'s own access can change *the barrel's own* diagnostic (its re-export
/// checked entry looks at `inner.ts` directly), but must never change `user.ts`'s.
#[test]
fn content_edit_of_inner_does_not_leak_through_an_explicit_passthrough_reexport() {
    let dir = TempDir::new().unwrap();
    let inner = canonical(&write(
        dir.path(),
        "src/inner.ts",
        "/** @public */\nexport const x = 1;\n",
    ));
    write(
        dir.path(),
        "src/sub/barrel.ts",
        "/** @public */\nexport { x } from \"../inner\";\n",
    );
    write(
        dir.path(),
        "other/user.ts",
        "import { x } from \"../src/sub/barrel\";\nconsole.log(x);\n",
    );

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert!(
        session.last_diagnostics().is_empty(),
        "expected a clean start, got {:?}",
        session.last_diagnostics()
    );

    // @public -> @private: the barrel's own re-export becomes a violation (private
    // is unconditional, no same-directory exception), but user.ts's own checked
    // entry only ever consults the barrel's own (unchanged, still @public)
    // export-table entry for "x" — it must stay clean.
    fs::write(&inner, "/** @private */\nexport const x = 1;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(inner)]);
    assert!(
        outcome
            .diagnostics
            .iter()
            .all(|d| !d.file.ends_with("user.ts")),
        "user.ts must not see a diagnostic from editing inner.ts through an explicit \
         passthrough re-export, got {:?}",
        outcome.diagnostics
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.file.ends_with("barrel.ts")),
        "expected barrel.ts's own re-export to now be flagged, got {:?}",
        outcome.diagnostics
    );
}

/// A content edit that changes only a file's *own* imports (not its export surface)
/// must not re-check anything but that one file (M7, PLAN.md §7's dirty-set
/// definition): `b.ts` gains an import of `a.ts` but its own exported `b` stays
/// untouched, so nothing that imports `b.ts` needs rechecking — there is nothing
/// importing `b.ts` here, but the assertion that matters is `rechecked_files == 1`,
/// not 2 (i.e. `a.ts` — which `b.ts` now imports — is not spuriously rechecked
/// either).
#[test]
fn content_edit_changing_only_its_own_imports_rechecks_only_itself() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");
    let b = canonical(&write(dir.path(), "src/b.ts", "export const b = 1;\n"));

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");

    fs::write(
        &b,
        "import { a } from \"./a\";\nexport const b = 1;\nconsole.log(a);\n",
    )
    .unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(b)]);
    assert_eq!(outcome.rechecked_files, 1);
    assert!(outcome.diagnostics.is_empty());
}

/// The fast path's own documented escape hatch (M7, PLAN.md §7's "locked design"
/// step 4): a content edit adds an import to a file that was never walked (it lives
/// outside the watched root, `src/`, so it was never part of the initial graph) —
/// the fast path can't reach a fixpoint for a brand-new graph node on its own and
/// must fall back to a full reload rather than panicking.
#[test]
fn content_edit_referencing_a_never_walked_file_falls_back_without_panicking() {
    let dir = TempDir::new().unwrap();
    let a = canonical(&write(dir.path(), "src/a.ts", "export const a = 1;\n"));
    write(
        dir.path(),
        "external/target.ts",
        "/** @public */\nexport const t = 1;\n",
    );

    let options = WatchSessionOptions {
        cli_paths: vec![dir.path().join("src")],
        explicit_config: None,
        cli_tsconfig: None,
        report_unresolved: false,
        quiet: false,
        cwd: dir.path().to_path_buf(),
    };
    let mut session = WatchSession::new(options).expect("session builds");
    assert!(session.last_diagnostics().is_empty());

    fs::write(
        &a,
        "import { t } from \"../external/target\";\nexport const a = 1;\nconsole.log(t);\n",
    )
    .unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(a)]);

    assert!(
        outcome.diagnostics.is_empty(),
        "target.ts is @public, so no violation is expected, got {:?}",
        outcome.diagnostics
    );
}

/// Span-insensitive surface comparison (M7, PLAN.md §7): moving `util.ts`'s JSDoc
/// comment down a line (inserting a leading blank line) shifts every span in the
/// file without changing which access level applies to `helper`. That must not
/// count as an export-surface change, so `consumer.ts` (which imports `helper` and
/// carries its own, unaffected, clean diagnostic — this fixture keeps the access
/// level allowed) is never added to the dirty set.
#[test]
fn moving_a_jsdoc_comment_without_changing_access_rechecks_only_the_edited_file() {
    let dir = TempDir::new().unwrap();
    let (_consumer, util) = write_violation_fixture(dir.path());

    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");
    assert_eq!(session.last_diagnostics().len(), 1);

    // Insert a leading blank line: every span in the file shifts, but the JSDoc
    // still immediately precedes the same declaration with the same tag.
    fs::write(&util, "\n/** @package */\nexport const helper = 1;\n").unwrap();
    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(util)]);

    assert_eq!(
        outcome.rechecked_files, 1,
        "only util.ts itself should be rechecked, not consumer.ts"
    );
    assert_eq!(
        outcome.diagnostics.len(),
        1,
        "consumer.ts's (unaffected) violation should still be reported from the cache"
    );
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

/// Watch single-edit cycle timing at 10k files (PLAN.md §8 perf target: < 100ms).
/// Ignored by default — generating a 10k-file synthetic tree and running the
/// initial full pipeline pass over it takes real time, and this is a perf
/// assertion rather than a correctness test. Run explicitly, in release mode (the
/// debug-build pipeline is far slower than 100ms even for a trivial cycle):
///
/// ```sh
/// cargo test --release -p import_lint_cli --test watch -- --ignored watch_cycle_timing_10k --nocapture
/// ```
#[test]
#[ignore]
fn watch_cycle_timing_10k() {
    let dir = TempDir::new().unwrap();
    let generated = gen_fixture::generate(
        dir.path(),
        &gen_fixture::GenOptions {
            files: 10_000,
            seed: 42,
        },
    )
    .expect("fixture generation should succeed");
    eprintln!(
        "generated {} files ({} content, {} barrels, {} ambient)",
        generated.total_files(),
        generated.content_files,
        generated.barrel_files,
        generated.ambient_files,
    );

    // The initial full run (walk + extract + link + check every file) happens in
    // `WatchSession::new` and isn't timed here — only the single-file incremental
    // edit cycle below is (that's the perf target this test asserts).
    let mut session = WatchSession::new(session_options(dir.path())).expect("session builds");

    let targets = import_lint_cli::walk(&[dir.path().to_path_buf()]);
    let target = targets
        .into_iter()
        .find(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name != "index.ts" && !name.ends_with(".d.ts")
        })
        .expect("gen-fixture should have produced at least one content file");

    let mut contents = fs::read_to_string(&target).unwrap();
    contents.push_str("\nexport const _watchTimingEdit = 1;\n");
    fs::write(&target, contents).unwrap();

    let outcome = session.run_cycle(&[ChangeKind::ContentEdit(target)]);

    eprintln!(
        "watch cycle: {:?} ({} files rechecked, {} re-extracted)",
        outcome.duration, outcome.rechecked_files, outcome.extracted_files
    );
    assert!(
        outcome.duration < Duration::from_millis(100),
        "watch cycle took {:?}, expected < 100ms (PLAN.md §8)",
        outcome.duration
    );
}
