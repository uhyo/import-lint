//! Watch mode (PLAN.md §7, M6 brief). [`WatchSession`] implements the update policy
//! (brief D-W1: full re-check every debounced batch, extraction cached; a fresh
//! `ProjectResolver` + re-walk only on a "structural" change) and is fully drivable
//! without `notify` (brief D-W3) — see `crates/cli/tests/watch.rs`. [`watch_loop`] is
//! the thin `notify`/`notify-debouncer-full` layer that maps real filesystem events
//! to [`ChangeKind`]s and drives a [`WatchSession`] from them (brief D-W2).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use import_lint::{LintConfig, ProjectResolver};
use notify::event::{AccessKind, AccessMode, MetadataKind, ModifyKind};
use notify::{Config as NotifyConfig, EventKind, PollWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, FileIdCache, NoCache, new_debouncer,
    new_debouncer_opt,
};
use oxc_allocator::AllocatorPool;

use crate::output::RenderedDiagnostic;
use crate::report::{ReportOptions, build_report};
use crate::runner::{self, ExtractionCache, RunnerOptions};
use crate::setup::{self, ConfigLoadError};
use crate::source_type::source_type_for_path;

/// One classified filesystem change, ready for [`WatchSession::run_cycle`] (M6 brief
/// D-W3). Pure data — no `notify` types leak into this enum, so session-level tests
/// (`crates/cli/tests/watch.rs`) can synthesize batches directly without spinning up
/// a real watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    /// An existing lintable file's content changed.
    ContentEdit(PathBuf),
    /// A file was created, removed, or renamed anywhere under a watched root, or a
    /// `package.json` changed anywhere — the filesystem layout may have shifted in a
    /// way that could change resolution results, so the walk and `ProjectResolver`
    /// are rebuilt from scratch (M6 brief D-W1).
    Structural,
    /// The `.importlintrc.jsonc`/`.importlintrc.json` config file changed.
    ConfigChanged,
    /// The project's `tsconfig.json` changed.
    TsconfigChanged,
}

/// Inputs [`WatchSession::new`] needs — the same CLI-flag inputs `main.rs`'s
/// `lint()` takes (see `crates/cli/src/setup.rs`), since a `ConfigChanged` cycle
/// redoes exactly that computation.
pub struct WatchSessionOptions {
    pub cli_paths: Vec<PathBuf>,
    pub explicit_config: Option<PathBuf>,
    pub cli_tsconfig: Option<PathBuf>,
    pub report_unresolved: bool,
    pub quiet: bool,
    pub cwd: PathBuf,
}

/// One completed cycle's result: everything a renderer or a test needs, plus the
/// counters the M6 brief's extraction-cache test relies on (D-W3).
pub struct CycleOutcome {
    pub diagnostics: Vec<RenderedDiagnostic>,
    pub has_error: bool,
    /// Number of lint targets actually re-checked this cycle (every lint target
    /// that has a `FileModuleInfo` — the check phase always re-runs over the full
    /// set each cycle, D-W1; this is "how big was that recheck", not a delta).
    pub rechecked_files: usize,
    /// Number of files actually re-parsed this cycle (extraction cache misses) —
    /// zero for a cycle with no relevant changes.
    pub extracted_files: usize,
    /// Every lint target this cycle, walked or not — the `json` formatter needs
    /// this to emit an entry for clean files too (ESLint's own behavior), matching
    /// the non-watch `lint()` path.
    pub linted_files: Vec<PathBuf>,
    pub duration: Duration,
    /// Set when this cycle included a `ConfigChanged` change whose reload failed
    /// (bad jsonc, unknown field, etc.). The *previous* config is kept and watching
    /// continues — this is a report string for the UI, never a fatal condition
    /// (M6 brief D-W1).
    pub config_error: Option<String>,
}

/// The paths `watch_loop` should have `notify` watch: each include root
/// (recursively), plus the config file, tsconfig, and project `package.json`
/// (non-recursively) if not already covered by a root (M6 brief D-W2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchTargets {
    pub roots: Vec<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub tsconfig_path: Option<PathBuf>,
    pub package_json_path: Option<PathBuf>,
}

impl WatchTargets {
    fn all_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.roots.clone();
        paths.extend(self.config_path.iter().cloned());
        paths.extend(self.tsconfig_path.iter().cloned());
        paths.extend(self.package_json_path.iter().cloned());
        paths
    }
}

/// Live watch-mode state (M6 brief D-W3): config, resolved options, the extraction
/// cache, the current walk result, and the built [`ProjectResolver`] —
/// [`run_cycle`](WatchSession::run_cycle) redoes the discovery+link+check pipeline
/// from these without ever touching `notify`.
pub struct WatchSession {
    cli_paths: Vec<PathBuf>,
    explicit_config: Option<PathBuf>,
    cli_tsconfig: Option<PathBuf>,
    report_unresolved: bool,
    quiet: bool,
    cwd: PathBuf,

    config: LintConfig,
    project_root: PathBuf,
    config_path: Option<PathBuf>,

    roots: Vec<PathBuf>,
    tsconfig_path: Option<PathBuf>,

    resolver: ProjectResolver,
    pool: AllocatorPool,
    extraction_cache: ExtractionCache,
    walked_paths: Vec<PathBuf>,

    last_diagnostics: Vec<RenderedDiagnostic>,
    last_has_error: bool,
}

/// Bundles what a "structural" (re-walk + fresh resolver) pass produces, so it can
/// be threaded straight into `extract_and_link_from` without a second extraction
/// pass over the same paths (see `runner.rs`'s doc comments on `extracted_files`
/// double-counting).
struct WalkAndResolver {
    roots: Vec<PathBuf>,
    tsconfig_path: Option<PathBuf>,
    resolver: ProjectResolver,
    walked_paths: Vec<PathBuf>,
    initial: runner::Extracted,
}

fn build_walk_and_resolver(
    cli_paths: &[PathBuf],
    cli_tsconfig: Option<&Path>,
    config: &LintConfig,
    project_root: &Path,
    pool: &AllocatorPool,
    cache: &mut ExtractionCache,
) -> WalkAndResolver {
    let roots = setup::compute_roots(cli_paths, config, project_root);
    let tsconfig_path = setup::compute_tsconfig(cli_tsconfig, config, project_root);
    let self_reference_mode = setup::compute_self_reference_mode(config);

    let runner_options = RunnerOptions {
        roots: roots.clone(),
        project_root: project_root.to_path_buf(),
        tsconfig: tsconfig_path.clone(),
        self_reference_mode,
        exclude: config.exclude.clone(),
    };

    let walked_paths = crate::walk::walk_with_excludes(
        &runner_options.roots,
        Some(runner_options.project_root.as_path()),
        &runner_options.exclude,
    );
    let initial = runner::extract_with_cache(&walked_paths, pool, cache);
    let resolver =
        runner::build_resolver_from_files(&runner_options, &walked_paths, &initial.files);

    WalkAndResolver {
        roots,
        tsconfig_path,
        resolver,
        walked_paths,
        initial,
    }
}

struct RecheckStats {
    rechecked_files: usize,
    extracted_files: usize,
    linted_files: Vec<PathBuf>,
}

impl WatchSession {
    /// Load the config, resolve options, and perform the initial full pipeline run
    /// (walk, extract, link, check) — `last_diagnostics`/`last_has_error` reflect
    /// the project's current state as soon as this returns. Fails only if the
    /// *initial* config can't be loaded (an explicit `--config` that doesn't exist,
    /// or a parse error): matching `main.rs`'s non-watch `lint()`, refusing to start
    /// watch mode on a broken config is the right call. Once running, a bad config
    /// edit is reported via `CycleOutcome::config_error` and the previous config is
    /// kept — never fatal (M6 brief D-W1) — see [`WatchSession::run_cycle`].
    pub fn new(options: WatchSessionOptions) -> Result<Self, ConfigLoadError> {
        let loaded = setup::load_config(options.explicit_config.as_deref(), &options.cwd)?;
        let pool = AllocatorPool::new(rayon::current_num_threads());
        let mut cache = ExtractionCache::new();

        let built = build_walk_and_resolver(
            &options.cli_paths,
            options.cli_tsconfig.as_deref(),
            &loaded.config,
            &loaded.project_root,
            &pool,
            &mut cache,
        );

        let mut session = WatchSession {
            cli_paths: options.cli_paths,
            explicit_config: options.explicit_config,
            cli_tsconfig: options.cli_tsconfig,
            report_unresolved: options.report_unresolved,
            quiet: options.quiet,
            cwd: options.cwd,

            config: loaded.config,
            project_root: loaded.project_root,
            config_path: loaded.config_path,

            roots: built.roots,
            tsconfig_path: built.tsconfig_path,

            resolver: built.resolver,
            pool,
            extraction_cache: cache,
            walked_paths: built.walked_paths,

            last_diagnostics: Vec::new(),
            last_has_error: false,
        };
        session.recheck(built.initial);
        Ok(session)
    }

    /// The most recently computed diagnostics (from `new()` or the last
    /// `run_cycle` call).
    pub fn last_diagnostics(&self) -> &[RenderedDiagnostic] {
        &self.last_diagnostics
    }

    pub fn last_has_error(&self) -> bool {
        self.last_has_error
    }

    /// Where `watch_loop` should point `notify` right now (M6 brief D-W2):
    /// recomputed on demand since `roots`/`config_path`/`tsconfig_path` can change
    /// after a `ConfigChanged`/`TsconfigChanged`/`Structural` reload.
    pub fn watch_targets(&self) -> WatchTargets {
        let package_json = self.project_root.join("package.json");
        let covered_by_a_root = self.roots.iter().any(|root| package_json.starts_with(root));
        WatchTargets {
            roots: self.roots.clone(),
            config_path: self.config_path.clone(),
            tsconfig_path: self.tsconfig_path.clone(),
            package_json_path: (!covered_by_a_root).then_some(package_json),
        }
    }

    /// Advance the session by one debounced batch of `changes` (M6 brief D-W1/D-W3):
    /// - `ConfigChanged`: reload the config; on success this also forces a full
    ///   reload (severity/options/include/exclude may have changed the walk itself);
    ///   on failure, keep the previous config and report the error via
    ///   `CycleOutcome::config_error` — watching continues either way.
    /// - `TsconfigChanged` or `Structural`: re-walk the roots and build a fresh
    ///   `ProjectResolver` (a resolver's cache assumes a frozen filesystem layout).
    /// - Otherwise (content-only batch): reuse the previous walk + resolver, just
    ///   drop the changed paths' extraction-cache entries (belt-and-braces on top of
    ///   the cache's own `mtime`/`size` staleness check — see `runner.rs`).
    ///
    /// Always ends with a full extract(-stale-only)+link+check pass and a re-render
    /// of every diagnostic (the check phase is cheap pure computation, D-W1) —
    /// `CycleOutcome::extracted_files` is the number of files that were *actually*
    /// re-parsed, which is 0 for a batch that touched nothing relevant.
    pub fn run_cycle(&mut self, changes: &[ChangeKind]) -> CycleOutcome {
        let start = Instant::now();
        let mut config_error = None;
        let mut need_full_reload = changes
            .iter()
            .any(|c| matches!(c, ChangeKind::TsconfigChanged | ChangeKind::Structural));

        if changes
            .iter()
            .any(|c| matches!(c, ChangeKind::ConfigChanged))
        {
            match setup::load_config(self.explicit_config.as_deref(), &self.cwd) {
                Ok(loaded) => {
                    self.config = loaded.config;
                    self.project_root = loaded.project_root;
                    self.config_path = loaded.config_path;
                    need_full_reload = true;
                }
                Err(err) => {
                    config_error = Some(err.to_string());
                }
            }
        }

        let initial = if need_full_reload {
            self.full_reload()
        } else {
            for change in changes {
                if let ChangeKind::ContentEdit(path) = change {
                    self.extraction_cache.remove(path);
                }
            }
            runner::extract_with_cache(&self.walked_paths, &self.pool, &mut self.extraction_cache)
        };

        let stats = self.recheck(initial);

        CycleOutcome {
            diagnostics: self.last_diagnostics.clone(),
            has_error: self.last_has_error,
            rechecked_files: stats.rechecked_files,
            extracted_files: stats.extracted_files,
            linted_files: stats.linted_files,
            duration: start.elapsed(),
            config_error,
        }
    }

    /// Re-walk `self.roots` (recomputed from the current config) and rebuild the
    /// resolver, returning the walked set's extraction (already served through
    /// `self.extraction_cache`) so the caller can feed it straight into
    /// [`WatchSession::recheck`].
    fn full_reload(&mut self) -> runner::Extracted {
        let built = build_walk_and_resolver(
            &self.cli_paths,
            self.cli_tsconfig.as_deref(),
            &self.config,
            &self.project_root,
            &self.pool,
            &mut self.extraction_cache,
        );
        self.roots = built.roots;
        self.tsconfig_path = built.tsconfig_path;
        self.resolver = built.resolver;
        self.walked_paths = built.walked_paths;
        built.initial
    }

    /// Run the fixpoint extract+link pass from `initial` and the check phase,
    /// updating `last_diagnostics`/`last_has_error`.
    fn recheck(&mut self, initial: runner::Extracted) -> RecheckStats {
        let outcome = runner::extract_and_link_from(
            &self.walked_paths,
            initial,
            &self.resolver,
            &self.pool,
            &mut self.extraction_cache,
        );
        let rechecked_files = crate::timing::phase("rechecked_files_count", || {
            outcome
                .graph
                .lint_targets
                .iter()
                .filter(|path| outcome.graph.file(path).is_some())
                .count()
        });
        let extracted_files = outcome.extracted_files;
        let linted_files: Vec<PathBuf> = crate::timing::phase("linted_files_clone", || {
            outcome.graph.lint_targets.iter().cloned().collect()
        });

        let report = crate::timing::phase("build_report_total", || {
            build_report(
                &outcome.graph,
                &self.config,
                &self.project_root,
                &ReportOptions {
                    report_unresolved: self.report_unresolved,
                    quiet: self.quiet,
                },
            )
        });
        self.last_diagnostics = report.diagnostics;
        self.last_has_error = report.has_error;

        RecheckStats {
            rechecked_files,
            extracted_files,
            linted_files,
        }
    }
}

/// Classify one debounced filesystem event into zero or more [`ChangeKind`]s (M6
/// brief D-W2), given the paths that currently have special meaning (`watch`). Pure
/// function of its inputs — unit-tested directly in `crates/cli/tests/watch.rs`
/// without a real watcher.
///
/// - `Create`/`Remove`/a name change (rename) anywhere -> `Structural`.
/// - `Modify(Metadata(WriteTime | Any))` is treated as a content change, not
///   ignored: `PollWatcher` (`--watch-poll`) has no OS-level notion of "data
///   changed" — a `stat()`-detected mtime bump is the *only* signal it emits for an
///   edited file, reported as `Modify(Metadata(WriteTime))` rather than
///   `Modify(Data(_))` (verified against notify 8.2.0's poll backend). Other
///   metadata-only touches (permissions, ownership, access time, xattrs) are
///   ignored — they can't affect linting.
/// - Otherwise (a data write, or the `Access(Close(Write))` event most editors and
///   atomic-save patterns produce — spike S5) each affected path is classified by
///   what it is: any `package.json` -> `Structural`; the config file ->
///   `ConfigChanged`; the tsconfig -> `TsconfigChanged`; a supported source
///   extension -> `ContentEdit`; anything else is ignored.
pub fn classify_event(event: &DebouncedEvent, watch: &WatchTargets) -> Vec<ChangeKind> {
    match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) => vec![ChangeKind::Structural],
        EventKind::Modify(ModifyKind::Name(_)) => vec![ChangeKind::Structural],
        EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Permissions | MetadataKind::Ownership | MetadataKind::AccessTime,
        )) => Vec::new(),
        EventKind::Modify(_) => event
            .paths
            .iter()
            .filter_map(|path| classify_path(path, watch))
            .collect(),
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => event
            .paths
            .iter()
            .filter_map(|path| classify_path(path, watch))
            .collect(),
        EventKind::Access(_) | EventKind::Other | EventKind::Any => Vec::new(),
    }
}

fn classify_path(path: &Path, watch: &WatchTargets) -> Option<ChangeKind> {
    if path.file_name().is_some_and(|name| name == "package.json") {
        return Some(ChangeKind::Structural);
    }
    if watch.config_path.as_deref() == Some(path) {
        return Some(ChangeKind::ConfigChanged);
    }
    if watch.tsconfig_path.as_deref() == Some(path) {
        return Some(ChangeKind::TsconfigChanged);
    }
    if source_type_for_path(path).is_some() {
        return Some(ChangeKind::ContentEdit(path.to_path_buf()));
    }
    None
}

/// Drive `session` from real filesystem events (M6 brief D-W2/D-W3): watches
/// `session`'s roots/config/tsconfig/package.json via `notify-debouncer-full`,
/// classifies each debounced batch into [`ChangeKind`]s, and calls
/// `session.run_cycle` — `on_cycle` is invoked with every [`CycleOutcome`]
/// (production code renders it to the terminal; tests assert on it directly, per the
/// M6 brief's "optional test hook").
///
/// `poll_interval` selects the watcher backend: `None` uses the platform-recommended
/// watcher (inotify on Linux); `Some(interval)` uses a `PollWatcher` at that interval
/// (`--watch-poll` — recommended for WSL2 Windows-side edits and network
/// filesystems, per `docs/research/spike-s5-watch-wsl2.md`).
///
/// Returns once `shutdown` is observed `true` (checked at least once per `debounce`
/// window via a receive timeout, so tests can stop the loop deterministically).
pub fn watch_loop(
    session: &mut WatchSession,
    debounce: Duration,
    poll_interval: Option<Duration>,
    shutdown: &Arc<AtomicBool>,
    mut on_cycle: impl FnMut(&CycleOutcome),
) -> notify::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<DebounceEventResult>();
    // Bounded so a shutdown request is never held up more than one poll behind the
    // debounce window itself — no point tying it to `debounce`, which can be large.
    let poll_shutdown_every = Duration::from_millis(200).min(debounce);

    match poll_interval {
        Some(interval) => {
            let config = NotifyConfig::default().with_poll_interval(interval);
            let mut debouncer =
                new_debouncer_opt::<_, PollWatcher, NoCache>(debounce, None, tx, NoCache, config)?;
            let mut watched = session.watch_targets();
            register_watches(&mut debouncer, &watched);
            run_event_loop(
                &mut debouncer,
                &rx,
                poll_shutdown_every,
                shutdown,
                session,
                &mut watched,
                &mut on_cycle,
            );
        }
        None => {
            let mut debouncer = new_debouncer(debounce, None, tx)?;
            let mut watched = session.watch_targets();
            register_watches(&mut debouncer, &watched);
            run_event_loop(
                &mut debouncer,
                &rx,
                poll_shutdown_every,
                shutdown,
                session,
                &mut watched,
                &mut on_cycle,
            );
        }
    }

    Ok(())
}

fn register_watches<T: Watcher, C: FileIdCache>(
    debouncer: &mut Debouncer<T, C>,
    targets: &WatchTargets,
) {
    for root in &targets.roots {
        if !root.exists() {
            eprintln!(
                "import-lint: {}: no such file or directory, not watching",
                root.display()
            );
            continue;
        }
        if let Err(err) = debouncer.watch(root, RecursiveMode::Recursive) {
            eprintln!("import-lint: failed to watch {}: {err}", root.display());
        }
    }
    for extra in [
        &targets.config_path,
        &targets.tsconfig_path,
        &targets.package_json_path,
    ]
    .into_iter()
    .flatten()
    {
        if !extra.is_file() {
            continue;
        }
        if let Err(err) = debouncer.watch(extra, RecursiveMode::NonRecursive) {
            eprintln!("import-lint: failed to watch {}: {err}", extra.display());
        }
    }
}

/// Best-effort: unwatch every path present in `old` but not `new`, and watch every
/// path present in `new` but not `old`. A `ConfigChanged`/`TsconfigChanged` reload
/// changing which roots/files matter is expected to be rare, so simplicity wins over
/// a precise diff (M6 brief D-W1's general stance — M7 will profile if this ever
/// matters).
fn reconcile_watches<T: Watcher, C: FileIdCache>(
    debouncer: &mut Debouncer<T, C>,
    old: &WatchTargets,
    new: &WatchTargets,
) {
    let new_paths = new.all_paths();
    for path in old.all_paths() {
        if !new_paths.contains(&path) {
            let _ = debouncer.unwatch(&path);
        }
    }
    let old_paths = old.all_paths();
    for path in new_paths {
        if old_paths.contains(&path) || !path.exists() {
            continue;
        }
        let mode = if new.roots.contains(&path) {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        let _ = debouncer.watch(&path, mode);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_event_loop<T: Watcher, C: FileIdCache>(
    debouncer: &mut Debouncer<T, C>,
    rx: &std::sync::mpsc::Receiver<DebounceEventResult>,
    recv_tick: Duration,
    shutdown: &Arc<AtomicBool>,
    session: &mut WatchSession,
    watched: &mut WatchTargets,
    on_cycle: &mut impl FnMut(&CycleOutcome),
) {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }

        match rx.recv_timeout(recv_tick) {
            Ok(Ok(events)) => {
                let changes: Vec<ChangeKind> = events
                    .iter()
                    .flat_map(|event| classify_event(event, watched))
                    .collect();
                if changes.is_empty() {
                    continue;
                }
                let outcome = session.run_cycle(&changes);
                on_cycle(&outcome);

                let new_watched = session.watch_targets();
                if new_watched != *watched {
                    reconcile_watches(debouncer, watched, &new_watched);
                    *watched = new_watched;
                }
            }
            Ok(Err(errors)) => {
                for err in errors {
                    eprintln!("import-lint: watch error: {err}");
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use notify::Event;
    use notify::event::{CreateKind, RemoveKind, RenameMode};
    use std::time::Instant;

    fn debounced(kind: EventKind, paths: &[&str]) -> DebouncedEvent {
        let mut event = Event::new(kind);
        for path in paths {
            event = event.add_path(PathBuf::from(path));
        }
        DebouncedEvent::new(event, Instant::now())
    }

    fn targets() -> WatchTargets {
        WatchTargets {
            roots: vec![PathBuf::from("/proj/src")],
            config_path: Some(PathBuf::from("/proj/.importlintrc.jsonc")),
            tsconfig_path: Some(PathBuf::from("/proj/tsconfig.json")),
            package_json_path: Some(PathBuf::from("/proj/package.json")),
        }
    }

    #[test]
    fn create_is_structural_regardless_of_path() {
        let event = debounced(EventKind::Create(CreateKind::File), &["/proj/src/new.ts"]);
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::Structural]
        );
    }

    #[test]
    fn remove_is_structural() {
        let event = debounced(EventKind::Remove(RemoveKind::File), &["/proj/src/gone.ts"]);
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::Structural]
        );
    }

    #[test]
    fn rename_is_structural() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/proj/src/old.ts", "/proj/src/new.ts"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::Structural]
        );
    }

    #[test]
    fn data_modify_of_a_source_file_is_content_edit() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            &["/proj/src/a.ts"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::ContentEdit(PathBuf::from("/proj/src/a.ts"))]
        );
    }

    #[test]
    fn close_write_of_a_source_file_is_content_edit() {
        let event = debounced(
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            &["/proj/src/a.ts"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::ContentEdit(PathBuf::from("/proj/src/a.ts"))]
        );
    }

    #[test]
    fn access_open_is_ignored() {
        let event = debounced(
            EventKind::Access(AccessKind::Open(AccessMode::Read)),
            &["/proj/src/a.ts"],
        );
        assert_eq!(classify_event(&event, &targets()), Vec::<ChangeKind>::new());
    }

    /// The `PollWatcher` backend has no OS-level notion of "data changed": it
    /// detects an edited file purely via a `stat()`-observed mtime bump, reported
    /// as `Modify(Metadata(WriteTime))` rather than `Modify(Data(_))` (verified
    /// against notify 8.2.0's poll backend — see the `--watch-poll` end-to-end
    /// smoke test). This must classify the same as a `Data` modify, or
    /// `--watch-poll` never detects any edit.
    #[test]
    fn metadata_write_time_modify_is_content_edit() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::WriteTime)),
            &["/proj/src/a.ts"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::ContentEdit(PathBuf::from("/proj/src/a.ts"))]
        );
    }

    #[test]
    fn metadata_permissions_modify_is_ignored() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Metadata(
                notify::event::MetadataKind::Permissions,
            )),
            &["/proj/src/a.ts"],
        );
        assert_eq!(classify_event(&event, &targets()), Vec::<ChangeKind>::new());
    }

    #[test]
    fn metadata_access_time_modify_is_ignored() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Metadata(
                notify::event::MetadataKind::AccessTime,
            )),
            &["/proj/src/a.ts"],
        );
        assert_eq!(classify_event(&event, &targets()), Vec::<ChangeKind>::new());
    }

    #[test]
    fn package_json_modify_is_structural_even_nested() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            &["/proj/src/nested/package.json"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::Structural]
        );
    }

    #[test]
    fn config_path_modify_is_config_changed() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            &["/proj/.importlintrc.jsonc"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::ConfigChanged]
        );
    }

    #[test]
    fn tsconfig_path_modify_is_tsconfig_changed() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            &["/proj/tsconfig.json"],
        );
        assert_eq!(
            classify_event(&event, &targets()),
            vec![ChangeKind::TsconfigChanged]
        );
    }

    #[test]
    fn modify_of_an_unrecognized_extension_is_ignored() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            &["/proj/src/readme.md"],
        );
        assert_eq!(classify_event(&event, &targets()), Vec::<ChangeKind>::new());
    }

    #[test]
    fn event_with_no_special_kind_is_ignored() {
        let event = debounced(EventKind::Other, &["/proj/src/a.ts"]);
        assert_eq!(classify_event(&event, &targets()), Vec::<ChangeKind>::new());
    }

    #[test]
    fn multiple_paths_classify_independently() {
        let event = debounced(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            &["/proj/src/a.ts", "/proj/tsconfig.json", "/proj/README.md"],
        );
        let mut got = classify_event(&event, &targets());
        got.sort_by_key(|c| format!("{c:?}"));
        let mut want = vec![
            ChangeKind::ContentEdit(PathBuf::from("/proj/src/a.ts")),
            ChangeKind::TsconfigChanged,
        ];
        want.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(got, want);
    }
}
