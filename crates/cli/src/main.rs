//! ImportLint CLI entry point.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand, ValueEnum};
use import_lint::diagnostics::line_col;
use import_lint::rule::SelfRefOpt;
use import_lint::{
    CheckedEntry, ConfigError, ExportInfo, FileModuleInfo, LintConfig, Provenance, Severity,
    check_graph, extract_file, find_config,
};
use import_lint_cli::output::{OutputSeverity, RenderedDiagnostic, eslint_json, github, pretty};
use import_lint_cli::runner::RunnerOptions;
use import_lint_cli::source_type::{SUPPORTED_EXTENSIONS_MESSAGE, source_type_for_path};
use oxc_allocator::Allocator;
use oxc_parser::Parser as OxcParser;
use oxc_str::CompactStr;
use serde::Serialize;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Pretty,
    Json,
    Github,
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Inspect { file }) => inspect(&file),
        Some(Command::Graph { paths, tsconfig }) => graph(paths, tsconfig),
        None => lint(cli),
    }
}

/// The default (no-subcommand) invocation: load config, run discovery + link +
/// check, and render diagnostics in the requested format (PLAN.md §5–§6, M5).
fn lint(cli: Cli) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("import-lint: cannot determine current directory: {err}");
            return ExitCode::from(2);
        }
    };

    let config_path = match &cli.config {
        Some(explicit) => {
            if !explicit.is_file() {
                eprintln!("import-lint: --config {}: no such file", explicit.display());
                return ExitCode::from(2);
            }
            Some(explicit.clone())
        }
        None => find_config(&cwd),
    };

    let (config, project_root) = match config_path {
        Some(path) => match LintConfig::load(&path) {
            Ok(config) => {
                let project_root = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| cwd.clone());
                (config, project_root)
            }
            Err(err) => {
                eprintln!("import-lint: {}", format_config_error(&err));
                return ExitCode::from(2);
            }
        },
        None => (LintConfig::default(), cwd.clone()),
    };

    let roots: Vec<PathBuf> = if cli.paths.is_empty() {
        config
            .include
            .iter()
            .map(|root| project_root.join(root))
            .collect()
    } else {
        cli.paths.clone()
    };

    let tsconfig = cli
        .tsconfig
        .clone()
        .or_else(|| config.tsconfig.as_ref().map(|path| project_root.join(path)))
        .or_else(|| RunnerOptions::default_tsconfig(&project_root));

    let self_reference_mode = match config.rules.jsdoc.options.treat_self_reference_as {
        SelfRefOpt::Internal => import_lint::SelfReferenceMode::Internal,
        SelfRefOpt::External => import_lint::SelfReferenceMode::External,
    };

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

    let mut source_cache: HashMap<PathBuf, String> = HashMap::new();
    let mut diagnostics: Vec<RenderedDiagnostic> = Vec::new();

    let severity = config.rules.jsdoc.severity;
    if severity != Severity::Off {
        let output_severity = match severity {
            Severity::Error => OutputSeverity::Error,
            Severity::Warn => OutputSeverity::Warn,
            Severity::Off => unreachable!("checked above"),
        };
        let core_diagnostics =
            check_graph(&module_graph, &config.rules.jsdoc.options, &project_root);
        for diagnostic in &core_diagnostics {
            let source = read_cached(&mut source_cache, &diagnostic.path);
            let (line, column) = line_col(source, diagnostic.span.start);
            let (end_line, end_column) = line_col(source, diagnostic.span.end);
            diagnostics.push(RenderedDiagnostic {
                file: diagnostic.path.clone(),
                line,
                column,
                end_line,
                end_column,
                severity: output_severity,
                rule_id: "import-access/jsdoc",
                message: diagnostic.message(),
                message_id: diagnostic.message_id.as_str().to_string(),
            });
        }
    }

    if cli.report_unresolved {
        collect_unresolved(&module_graph, &mut source_cache, &mut diagnostics);
    }

    diagnostics.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));

    let has_error = diagnostics
        .iter()
        .any(|d| d.severity == OutputSeverity::Error);

    if cli.quiet {
        diagnostics.retain(|d| d.severity != OutputSeverity::Warn);
    }

    let stdout = io::stdout();
    let colors = cli.format == OutputFormat::Pretty && stdout.is_terminal();
    let mut handle = stdout.lock();
    let render_result = match cli.format {
        OutputFormat::Pretty => pretty::render(&mut handle, &diagnostics, &cwd, colors),
        OutputFormat::Json => {
            let linted_files: Vec<PathBuf> = module_graph.lint_targets.iter().cloned().collect();
            eslint_json::render(&mut handle, &diagnostics, &linted_files)
        }
        OutputFormat::Github => github::render(&mut handle, &diagnostics, &cwd),
    };
    if let Err(err) = render_result.and_then(|()| handle.flush()) {
        eprintln!("import-lint: failed to write output: {err}");
        return ExitCode::from(2);
    }

    if has_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn format_config_error(err: &ConfigError) -> String {
    format!("failed to load config: {err}")
}

fn read_cached<'a>(cache: &'a mut HashMap<PathBuf, String>, path: &Path) -> &'a str {
    cache
        .entry(path.to_path_buf())
        .or_insert_with(|| fs::read_to_string(path).unwrap_or_default())
}

/// `--report-unresolved`: emit a warn-severity diagnostic for every checked entry
/// whose specifier failed to resolve (D8's opt-in debug aid). These never affect
/// the exit code (M5 brief §3).
fn collect_unresolved(
    graph: &import_lint::ModuleGraph,
    source_cache: &mut HashMap<PathBuf, String>,
    diagnostics: &mut Vec<RenderedDiagnostic>,
) {
    for target in &graph.lint_targets {
        let Some(file) = graph.file(target) else {
            continue;
        };
        for entry in &file.checked_entries {
            if !matches!(
                graph.resolution(target, &entry.specifier),
                Some(Provenance::Unresolved)
            ) {
                continue;
            }
            let source = read_cached(source_cache, target);
            let (line, column) = line_col(source, entry.span.start);
            let (end_line, end_column) = line_col(source, entry.span.end);
            diagnostics.push(RenderedDiagnostic {
                file: target.clone(),
                line,
                column,
                end_line,
                end_column,
                severity: OutputSeverity::Warn,
                rule_id: "import-access/unresolved",
                message: format!("Unresolved import specifier '{}'", entry.specifier),
                message_id: "unresolved".to_string(),
            });
        }
    }
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
