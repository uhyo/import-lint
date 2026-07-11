//! ImportLint CLI entry point.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand};
use import_lint::diagnostics::line_col;
use import_lint::{
    CheckedEntry, Diagnostic, ExportInfo, FileModuleInfo, JsdocRuleOptions, Provenance,
    check_graph, extract_file,
};
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

    /// Paths to lint (default: the current directory).
    paths: Vec<PathBuf>,

    /// Path to the project's `tsconfig.json` (default: `./tsconfig.json` if it
    /// exists).
    #[arg(long)]
    tsconfig: Option<PathBuf>,
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
        None => lint(cli.paths, cli.tsconfig),
    }
}

/// The default (no-subcommand) invocation: run discovery + link + check over
/// `paths` (default: `.`) and print a human-readable diagnostic report.
fn lint(paths: Vec<PathBuf>, tsconfig: Option<PathBuf>) -> ExitCode {
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
        project_root: project_root.clone(),
        tsconfig,
        self_reference_mode: import_lint::SelfReferenceMode::default(),
    };
    let module_graph = import_lint_cli::run(&options);

    let diagnostics = check_graph(&module_graph, &JsdocRuleOptions::default(), &project_root);

    if diagnostics.is_empty() {
        return ExitCode::SUCCESS;
    }

    let mut source_cache: HashMap<PathBuf, String> = HashMap::new();
    for diagnostic in &diagnostics {
        print_diagnostic(diagnostic, &project_root, &mut source_cache);
    }
    println!();
    println!(
        "\u{2716} {} problem{}",
        diagnostics.len(),
        if diagnostics.len() == 1 { "" } else { "s" }
    );

    ExitCode::from(1)
}

/// Print one diagnostic in the form `path:line:col error <message> (import-access/jsdoc)`,
/// with `path` relative to `project_root` when possible. Reads (and caches) each
/// diagnosed file's source once, to compute line/column from the diagnostic's byte
/// span.
fn print_diagnostic(
    diagnostic: &Diagnostic,
    project_root: &Path,
    source_cache: &mut HashMap<PathBuf, String>,
) {
    let source = source_cache
        .entry(diagnostic.path.clone())
        .or_insert_with(|| fs::read_to_string(&diagnostic.path).unwrap_or_default());
    let (line, column) = line_col(source, diagnostic.span.start);

    let display_path = diagnostic
        .path
        .strip_prefix(project_root)
        .unwrap_or(&diagnostic.path);

    println!(
        "{}:{line}:{column} error {} (import-access/jsdoc)",
        display_path.display(),
        diagnostic.message(),
    );
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
