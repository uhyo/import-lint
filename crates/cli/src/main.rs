//! ImportLint CLI entry point.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use clap::{Parser as ClapParser, Subcommand};
use import_lint::{CheckedEntry, ExportInfo, FileModuleInfo, Provenance, extract_file};
use import_lint_cli::lsp::{self, LspOptions};
use import_lint_cli::output::{OutputFormat, RenderedDiagnostic};
use import_lint_cli::overlay::Overlays;
use import_lint_cli::report::{ReportOptions, build_report};
use import_lint_cli::runner::RunnerOptions;
use import_lint_cli::setup;
use import_lint_cli::source_type::{SUPPORTED_EXTENSIONS_MESSAGE, source_type_for_path};
use import_lint_cli::watch::{CycleOutcome, WatchSession, WatchSessionOptions, watch_loop};
use oxc_allocator::Allocator;
use oxc_parser::Parser as OxcParser;
use oxc_str::CompactStr;
use serde::Serialize;

mod init;

/// A Rust CLI linter that checks module-boundary import access (JSDoc `@package`/`@private`).
#[derive(ClapParser, Debug)]
#[command(name = "import-lint", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Paths to lint (default: config `include`, or the current directory with no
    /// config). Overrides the config file's `include` when given.
    paths: Vec<PathBuf>,

    /// Path to an explicit `.importlintrc.jsonc`/`.importlintrc.json` config file
    /// (default: discovered by walking up from the current directory).
    #[arg(long)]
    config: Option<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
    format: OutputFormat,

    /// Rayon thread pool size (default: number of cores).
    #[arg(long)]
    threads: Option<usize>,

    /// Path to the project's `tsconfig.json` (overrides the config file; default:
    /// `<project root>/tsconfig.json` if it exists).
    #[arg(long)]
    tsconfig: Option<PathBuf>,

    /// Emit a warning for every import specifier that fails to resolve, instead of
    /// skipping it silently.
    #[arg(long)]
    report_unresolved: bool,

    /// Suppress warning-severity output (errors only), like `eslint --quiet`.
    #[arg(long)]
    quiet: bool,

    /// Watch mode: re-run whenever a watched file changes (PLAN-v1.md §7). Uses the
    /// platform-recommended watcher (inotify on Linux).
    #[arg(long)]
    watch: bool,

    /// Watch mode using a polling watcher instead of the platform-recommended one.
    /// Implies `--watch`. Recommended when editing from the Windows side on WSL2, or
    /// over a network filesystem (NFS) — see README. Optional poll interval in
    /// milliseconds (default: 500).
    #[arg(
        long,
        value_name = "INTERVAL_MS",
        num_args = 0..=1,
        default_missing_value = "500"
    )]
    watch_poll: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Parse a single file and print its extracted module info as JSON. A debug aid
    /// for developing the extraction and rule-engine phases.
    Inspect {
        /// The file to inspect.
        file: PathBuf,
    },
    /// Run discovery + the link phase and print the resulting module graph as JSON.
    /// A debug aid for developing the pipeline and rule-engine phases.
    Graph {
        /// Root paths to walk (default: the current directory).
        paths: Vec<PathBuf>,
        /// Path to the project's `tsconfig.json` (default: `./tsconfig.json` if it
        /// exists).
        #[arg(long)]
        tsconfig: Option<PathBuf>,
    },
    /// Run the LSP server (stdio).
    Lsp,
    /// Scaffold a `.importlintrc.jsonc` into the current directory, which
    /// thereby becomes the project root (M9, `docs/PLAN-init.md`).
    Init {
        /// Overwrite an existing `.importlintrc.jsonc`/`.importlintrc.json` in
        /// the current directory.
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Inspect { file }) => inspect(&file),
        Some(Command::Graph { paths, tsconfig }) => graph(paths, tsconfig),
        Some(Command::Lsp) => lsp_command(),
        Some(Command::Init { force }) => init_command(force),
        None if cli.watch || cli.watch_poll.is_some() => watch_command(cli),
        None => lint(cli),
    }
}

/// The default (no-subcommand) invocation: load config, run discovery + link +
/// check, and render diagnostics in the requested format (PLAN-v1.md §5–§6, M5).
fn lint(cli: Cli) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("import-lint: cannot determine current directory: {err}");
            return ExitCode::from(2);
        }
    };

    let loaded = match setup::load_config(cli.config.as_deref(), &cwd) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("import-lint: {err}");
            return ExitCode::from(2);
        }
    };
    let config = loaded.config;
    let project_root = loaded.project_root;

    let roots = setup::compute_roots(&cli.paths, &config, &project_root);
    let tsconfig = setup::compute_tsconfig(cli.tsconfig.as_deref(), &config, &project_root);
    let self_reference_mode = setup::compute_self_reference_mode(&config);

    if let Some(threads) = cli.threads {
        // Ignore the error: it only fails if a global pool was already built (e.g.
        // by a caller embedding this binary's logic), which just means the
        // existing pool is used instead.
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global();
    }

    let runner_options = RunnerOptions {
        roots,
        project_root: project_root.clone(),
        tsconfig,
        self_reference_mode,
        exclude: config.exclude.clone(),
    };
    let module_graph = import_lint_cli::run(&runner_options);

    let report = build_report(
        &module_graph,
        &config,
        &project_root,
        &ReportOptions {
            report_unresolved: cli.report_unresolved,
            quiet: cli.quiet,
        },
        &Overlays::default(),
    );

    let stdout = io::stdout();
    let colors = cli.format == OutputFormat::Pretty && stdout.is_terminal();
    let mut handle = stdout.lock();
    let linted_files: Vec<PathBuf> = module_graph.lint_targets.iter().cloned().collect();
    let render_result = cli.format.render(
        &mut handle,
        &report.diagnostics,
        &cwd,
        colors,
        &linted_files,
    );
    if let Err(err) = render_result.and_then(|()| handle.flush()) {
        eprintln!("import-lint: failed to write output: {err}");
        return ExitCode::from(2);
    }

    if report.has_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// The `--watch`/`--watch-poll` invocation: build a `WatchSession` (which performs
/// the initial full run), render its current state, then hand off to `watch_loop`
/// forever — real filesystem events drive `WatchSession::run_cycle` from here on
/// (PLAN-v1.md §7, M6). Exits 2 only on a startup failure (bad `--config`, watcher
/// setup failure); otherwise watch mode runs until the process is killed (default
/// SIGINT behavior — no handler needed, M6 brief D-W4).
fn watch_command(cli: Cli) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("import-lint: cannot determine current directory: {err}");
            return ExitCode::from(2);
        }
    };

    if let Some(threads) = cli.threads {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global();
    }

    let format = cli.format;
    let poll_interval = cli.watch_poll.map(Duration::from_millis);

    let session_options = WatchSessionOptions {
        cli_paths: cli.paths.clone(),
        explicit_config: cli.config.clone(),
        cli_tsconfig: cli.tsconfig.clone(),
        report_unresolved: cli.report_unresolved,
        quiet: cli.quiet,
        cwd: cwd.clone(),
    };

    let mut session = match WatchSession::new(session_options) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("import-lint: {err}");
            return ExitCode::from(2);
        }
    };

    let tty = io::stdout().is_terminal();
    render_watch_state(session.last_diagnostics(), None, format, &cwd, tty);

    // No shutdown signal is ever set from here: watch mode exits via the default
    // (unhandled) SIGINT behavior, matching the M6 brief's "no handler needed".
    // `shutdown` exists purely so `watch_loop` is drivable/stoppable from tests
    // (`crates/cli/tests/watch.rs`) without relying on a real signal.
    let shutdown = Arc::new(AtomicBool::new(false));
    let debounce = Duration::from_millis(100);

    let result = watch_loop(
        &mut session,
        debounce,
        poll_interval,
        &shutdown,
        |outcome| {
            render_watch_cycle(outcome, format, &cwd, tty);
        },
    );

    if let Err(err) = result {
        eprintln!("import-lint: watch mode failed to start: {err}");
        return ExitCode::from(2);
    }

    ExitCode::SUCCESS
}

/// The `lsp` subcommand (M8/L2, `docs/PLAN-lsp.md` §2): hand off to
/// `import_lint_cli::lsp::run_stdio` and translate its result into an exit code.
/// Takes no arguments — the LSP client configures everything (workspace root,
/// run mode) through the protocol itself.
fn lsp_command() -> ExitCode {
    match lsp::run_stdio(LspOptions::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("import-lint: lsp server failed: {err}");
            ExitCode::from(2)
        }
    }
}

/// The `init` subcommand (M9, `docs/PLAN-init.md` D-I1): scaffold a
/// `.importlintrc.jsonc` into the current directory. Fully non-interactive —
/// there is exactly one template, so this works the same in a terminal, a
/// script, or CI.
fn init_command(force: bool) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("import-lint: cannot determine current directory: {err}");
            return ExitCode::from(2);
        }
    };

    match init::run_init(&cwd, force) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("import-lint: {err}");
            ExitCode::from(2)
        }
    }
}

/// Render one watch-mode cycle (M6 brief D-W4): a full re-render each time (no
/// incremental diffing), preceded by an ANSI clear-screen when stdout is a TTY and
/// the format is `pretty` — `json`/`github` and non-TTY output just re-print in
/// full, so piping/redirecting `import-lint --watch` produces a readable transcript.
fn render_watch_cycle(outcome: &CycleOutcome, format: OutputFormat, cwd: &Path, tty: bool) {
    let status = watch_status_line(outcome);
    render_watch_state(
        &outcome.diagnostics,
        Some((&outcome.linted_files, &status)),
        format,
        cwd,
        tty,
    );
}

fn watch_status_line(outcome: &CycleOutcome) -> String {
    let error_count = outcome
        .diagnostics
        .iter()
        .filter(|d| d.severity == import_lint_cli::output::OutputSeverity::Error)
        .count();
    let warning_count = outcome.diagnostics.len() - error_count;
    let millis = outcome.duration.as_millis();
    let tail = format!(
        "rechecked {} file{} in {millis} ms (watching, Ctrl-C to exit)",
        outcome.rechecked_files,
        if outcome.rechecked_files == 1 {
            ""
        } else {
            "s"
        }
    );
    if outcome.has_error || warning_count > 0 {
        let total = outcome.diagnostics.len();
        format!(
            "\u{2716} {total} problem{} ({error_count} error{}, {warning_count} warning{}) \u{2014} {tail}",
            if total == 1 { "" } else { "s" },
            if error_count == 1 { "" } else { "s" },
            if warning_count == 1 { "" } else { "s" },
        )
    } else {
        format!("\u{2713} no problems \u{2014} {tail}")
    }
}

/// Shared by the initial render (`status = None`, no clear) and every subsequent
/// cycle (`status = Some((linted_files, status_line))`).
fn render_watch_state(
    diagnostics: &[RenderedDiagnostic],
    status: Option<(&[PathBuf], &str)>,
    format: OutputFormat,
    cwd: &Path,
    tty: bool,
) {
    let colors = tty && format == OutputFormat::Pretty;
    let clear = tty && format == OutputFormat::Pretty && status.is_some();
    let linted_files: &[PathBuf] = status.map(|(files, _)| files).unwrap_or(&[]);

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if clear {
        let _ = write!(handle, "\x1b[2J\x1b[H");
    }
    let _ = format.render(&mut handle, diagnostics, cwd, colors, linted_files);
    if let Some((_, status_line)) = status {
        let styled = if colors {
            format!("\x1b[1m{status_line}\x1b[0m")
        } else {
            status_line.to_string()
        };
        let _ = writeln!(handle, "{styled}");
    }
    let _ = handle.flush();
}

fn inspect(file: &Path) -> ExitCode {
    let Some(source_type) = source_type_for_path(file) else {
        eprintln!(
            "import-lint: {}: unrecognized file extension ({SUPPORTED_EXTENSIONS_MESSAGE})",
            file.display()
        );
        return ExitCode::from(2);
    };

    let source_text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("import-lint: cannot read {}: {err}", file.display());
            return ExitCode::from(2);
        }
    };

    // Pre-flight parse to surface syntax errors with exit code 2, matching the CLI
    // contract. `extract_file` below does its own (separate) parse — an accepted
    // duplication for a single-file debug command, in exchange for keeping the core
    // `extract()` entry point a plain parse-and-extract function with no diagnostics
    // in its return type.
    let preflight_allocator = Allocator::default();
    let preflight = OxcParser::new(&preflight_allocator, &source_text, source_type).parse();
    if preflight.panicked || preflight.diagnostics.has_errors() {
        eprintln!("import-lint: failed to parse {}:", file.display());
        for diagnostic in preflight.diagnostics.errors() {
            eprintln!("  {diagnostic}");
        }
        return ExitCode::from(2);
    }

    let allocator = Allocator::default();
    let info = extract_file(file, &source_text, source_type, &allocator);

    match serde_json::to_string_pretty(&InspectOutput::from(&info)) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("import-lint: failed to serialize extraction output: {err}");
            ExitCode::from(2)
        }
    }
}

/// A `FileModuleInfo` view for JSON output with a deterministic `export_table` key
/// order (a `BTreeMap`, since `FileModuleInfo::export_table` is a `HashMap` whose
/// iteration order is not stable across runs).
#[derive(Serialize)]
struct InspectOutput<'a> {
    path: &'a Path,
    checked_entries: &'a [CheckedEntry],
    export_table: BTreeMap<&'a str, &'a ExportInfo>,
    star_exports: &'a [CompactStr],
    ambient_modules: &'a [CompactStr],
    specifiers: &'a [CompactStr],
}

impl<'a> From<&'a FileModuleInfo> for InspectOutput<'a> {
    fn from(info: &'a FileModuleInfo) -> Self {
        Self {
            path: &info.path,
            checked_entries: &info.checked_entries,
            export_table: info
                .export_table
                .iter()
                .map(|(k, v)| (k.as_str(), v))
                .collect(),
            star_exports: &info.star_exports,
            ambient_modules: &info.ambient_modules,
            specifiers: &info.specifiers,
        }
    }
}

fn graph(paths: Vec<PathBuf>, tsconfig: Option<PathBuf>) -> ExitCode {
    let roots = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };

    let project_root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("import-lint: cannot determine current directory: {err}");
            return ExitCode::from(2);
        }
    };
    let tsconfig = tsconfig.or_else(|| RunnerOptions::default_tsconfig(&project_root));

    let options = RunnerOptions {
        roots,
        project_root,
        tsconfig,
        self_reference_mode: import_lint::SelfReferenceMode::default(),
        exclude: Vec::new(),
    };
    let module_graph = import_lint_cli::run(&options);

    match serde_json::to_string_pretty(&GraphOutput::from(&module_graph)) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("import-lint: failed to serialize graph output: {err}");
            ExitCode::from(2)
        }
    }
}

/// A `Provenance` view for JSON output: `{"kind": "internal", "path": "..."}`,
/// `{"kind": "external"}`, or `{"kind": "unresolved"}`.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResolutionOutput {
    Internal { path: String },
    External,
    Unresolved,
}

impl From<&Provenance> for ResolutionOutput {
    fn from(provenance: &Provenance) -> Self {
        match provenance {
            Provenance::Internal(path) => ResolutionOutput::Internal {
                path: path.to_string_lossy().into_owned(),
            },
            Provenance::External => ResolutionOutput::External,
            Provenance::Unresolved => ResolutionOutput::Unresolved,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOutput {
    lint_target: bool,
    resolutions: BTreeMap<String, ResolutionOutput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    files: usize,
    lint_targets: usize,
    internal: usize,
    external: usize,
    unresolved: usize,
}

/// A `ModuleGraph` view for JSON output with deterministic (`BTreeMap`) key order,
/// mirroring `InspectOutput`'s treatment of `FileModuleInfo::export_table`.
#[derive(Serialize)]
struct GraphOutput {
    files: BTreeMap<String, FileOutput>,
    summary: Summary,
}

impl From<&import_lint::ModuleGraph> for GraphOutput {
    fn from(graph: &import_lint::ModuleGraph) -> Self {
        let mut files = BTreeMap::new();
        let mut internal = 0;
        let mut external = 0;
        let mut unresolved = 0;

        for (path, file) in &graph.files {
            let mut resolutions = BTreeMap::new();
            for specifier in &file.specifiers {
                if let Some(provenance) = graph.resolution(path, specifier) {
                    resolutions.insert(specifier.to_string(), ResolutionOutput::from(provenance));
                    match provenance {
                        Provenance::Internal(_) => internal += 1,
                        Provenance::External => external += 1,
                        Provenance::Unresolved => unresolved += 1,
                    }
                }
            }
            files.insert(
                path.to_string_lossy().into_owned(),
                FileOutput {
                    lint_target: graph.lint_targets.contains(path),
                    resolutions,
                },
            );
        }

        GraphOutput {
            summary: Summary {
                files: graph.files.len(),
                lint_targets: graph.lint_targets.len(),
                internal,
                external,
                unresolved,
            },
            files,
        }
    }
}
