//! ImportLint CLI entry point.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand};
use import_lint::{CheckedEntry, ExportInfo, FileModuleInfo, extract_file};
use oxc_allocator::Allocator;
use oxc_parser::Parser as OxcParser;
use oxc_span::SourceType;
use oxc_str::CompactStr;
use serde::Serialize;

/// A Rust CLI linter that checks module-boundary import access (JSDoc `@package`/`@private`).
#[derive(ClapParser, Debug)]
#[command(name = "import-lint", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Paths to lint (overrides the configured include roots). Not yet implemented.
    paths: Vec<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Parse a single file and print its extracted module info as JSON. A debug aid
    /// for developing the extraction and rule-engine phases.
    Inspect {
        /// The file to inspect.
        file: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Inspect { file }) => inspect(&file),
        None => {
            let _ = cli.paths;
            println!("import-lint: not yet implemented");
            ExitCode::SUCCESS
        }
    }
}

fn inspect(file: &Path) -> ExitCode {
    let Ok(source_type) = SourceType::from_path(file) else {
        eprintln!(
            "import-lint: {}: unrecognized file extension (expected one of .js, .mjs, .cjs, .jsx, .ts, .mts, .cts, .tsx, .d.ts, .d.mts, .d.cts)",
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
