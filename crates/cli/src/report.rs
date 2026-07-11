//! Turning a checked [`ModuleGraph`] into the [`RenderedDiagnostic`]s a formatter
//! renders (PLAN.md §2.1 step 5–6, §5 `--report-unresolved`/`--quiet`). Factored out
//! of `main.rs`'s `lint()` (M5) so watch mode (`crates/cli/src/watch.rs`, M6) can
//! rebuild the same report every cycle without duplicating the logic.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use import_lint::diagnostics::line_col;
use import_lint::{LintConfig, ModuleGraph, Provenance, Severity, check_graph};

use crate::output::{OutputSeverity, RenderedDiagnostic};
use crate::timing;

/// Flags that affect what gets reported, orthogonal to the rule engine itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReportOptions {
    /// D8's opt-in debug aid: emit a warn-severity diagnostic for every checked
    /// entry whose specifier failed to resolve.
    pub report_unresolved: bool,
    /// Suppress warning-severity output (errors only), like `eslint --quiet`.
    /// Applied after `has_error` is computed, so `--quiet` never changes the exit
    /// code (M5 brief §3).
    pub quiet: bool,
}

/// The full result of one report pass: diagnostics ready to render, plus whether any
/// of them are error-severity (before `--quiet` filtering — see [`ReportOptions::quiet`]).
pub struct ReportResult {
    pub diagnostics: Vec<RenderedDiagnostic>,
    pub has_error: bool,
}

/// Run the rule engine over `module_graph` under `config`, plus `--report-unresolved`
/// if requested, and produce the final sorted, `--quiet`-filtered diagnostic list.
/// Source files are read once per file (`source_cache`) to compute line/column from
/// byte-offset spans.
pub fn build_report(
    module_graph: &ModuleGraph,
    config: &LintConfig,
    project_root: &Path,
    options: &ReportOptions,
) -> ReportResult {
    let mut source_cache: HashMap<PathBuf, String> = HashMap::new();
    let mut diagnostics: Vec<RenderedDiagnostic> = Vec::new();

    let severity = config.rules.jsdoc.severity;
    if severity != Severity::Off {
        let output_severity = match severity {
            Severity::Error => OutputSeverity::Error,
            Severity::Warn => OutputSeverity::Warn,
            Severity::Off => unreachable!("checked above"),
        };
        let core_diagnostics = timing::phase("check_graph", || {
            check_graph(module_graph, &config.rules.jsdoc.options, project_root)
        });
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

    if options.report_unresolved {
        collect_unresolved(module_graph, &mut source_cache, &mut diagnostics);
    }

    diagnostics.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));

    let has_error = diagnostics
        .iter()
        .any(|d| d.severity == OutputSeverity::Error);

    if options.quiet {
        diagnostics.retain(|d| d.severity != OutputSeverity::Warn);
    }

    ReportResult {
        diagnostics,
        has_error,
    }
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
    graph: &ModuleGraph,
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
