//! Turning a checked [`ModuleGraph`] into the [`RenderedDiagnostic`]s a formatter
//! renders (PLAN-v1.md §2.1 step 5–6, §5 `--report-unresolved`/`--quiet`). Factored out
//! of `main.rs`'s `lint()` (M5) so watch mode (`crates/cli/src/watch.rs`, M6) can
//! rebuild the same report every cycle without duplicating the logic.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use import_lint::diagnostics::line_col;
use import_lint::{LintConfig, ModuleGraph, Provenance, Severity, check_files};

use crate::output::{OutputSeverity, RenderedDiagnostic};
use crate::overlay::Overlays;
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
/// byte-offset spans — an overlay's content wins over disk for any file `overlays`
/// covers (L1, `docs/PLAN-lsp.md` §3); non-watch callers pass `&Overlays::default()` for
/// no behavior change.
pub fn build_report(
    module_graph: &ModuleGraph,
    config: &LintConfig,
    project_root: &Path,
    options: &ReportOptions,
    overlays: &Overlays,
) -> ReportResult {
    let targets: Vec<&Path> = module_graph
        .lint_targets
        .iter()
        .map(PathBuf::as_path)
        .collect();
    let by_file = timing::phase("check_graph", || {
        diagnostics_by_file(
            module_graph,
            config,
            project_root,
            options,
            &targets,
            overlays,
        )
    });
    finish_report(by_file.into_values().flatten(), options)
}

/// Sort and `--quiet`-filter a flat stream of diagnostics into the same shape
/// [`build_report`] has always produced. Takes an iterator rather than the
/// `diagnostics_by_file` map directly so `crates/cli/src/watch.rs`'s incremental fast
/// path can compose its *persistent* per-file map (`self.diagnostics_map`) into a
/// `CycleOutcome` by borrowing and cloning just the diagnostics themselves
/// (`self.diagnostics_map.values().flatten().cloned()`), without cloning the whole
/// map every cycle — the entire point of keeping a per-file map across cycles.
pub fn finish_report(
    diagnostics: impl IntoIterator<Item = RenderedDiagnostic>,
    options: &ReportOptions,
) -> ReportResult {
    let mut diagnostics: Vec<RenderedDiagnostic> = diagnostics.into_iter().collect();
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

/// Compute diagnostics restricted to `files`, grouped by file (each file's own list
/// sorted by position) — the per-file counterpart to [`build_report`]. Every path in
/// `files` is guaranteed to appear as a key, with an empty `Vec` if it turned out
/// clean, so a caller can always replace old map entries with this result's entries
/// wholesale without leaving a stale diagnostic behind for a file that's no longer
/// dirty.
///
/// Every diagnostic (both a `check_files` rule violation and a `--report-unresolved`
/// note) is self-attributed to the importer file it's about — see
/// [`check_files`]'s one-hop doc comment and [`collect_unresolved`]'s `target` loop
/// variable — so grouping by `diagnostic.file`/`diagnostic.path` is exact: nothing
/// about a file `f`'s entry here depends on any *other* lint target's own
/// diagnostics. This is what makes watch mode's incremental fast path
/// (`crates/cli/src/watch.rs`, PLAN-v1.md §7) correct: recomputing just the dirty subset
/// and merging into a persistent map gives the same result as recomputing everything.
pub fn diagnostics_by_file(
    module_graph: &ModuleGraph,
    config: &LintConfig,
    project_root: &Path,
    options: &ReportOptions,
    files: &[&Path],
    overlays: &Overlays,
) -> HashMap<PathBuf, Vec<RenderedDiagnostic>> {
    let mut source_cache: HashMap<PathBuf, String> = HashMap::new();
    let mut by_file: HashMap<PathBuf, Vec<RenderedDiagnostic>> = files
        .iter()
        .map(|file| (file.to_path_buf(), Vec::new()))
        .collect();

    let severity = config.rules.jsdoc.severity;
    if severity != Severity::Off {
        let output_severity = match severity {
            Severity::Error => OutputSeverity::Error,
            Severity::Warn => OutputSeverity::Warn,
            Severity::Off => unreachable!("checked above"),
        };
        let core_diagnostics = check_files(
            module_graph,
            &config.rules.jsdoc.options,
            project_root,
            files,
        );
        for diagnostic in &core_diagnostics {
            let source = read_cached(&mut source_cache, &diagnostic.path, overlays);
            let (line, column) = line_col(source, diagnostic.span.start);
            let (end_line, end_column) = line_col(source, diagnostic.span.end);
            by_file
                .entry(diagnostic.path.clone())
                .or_default()
                .push(RenderedDiagnostic {
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
        collect_unresolved(
            module_graph,
            files,
            &mut source_cache,
            &mut by_file,
            overlays,
        );
    }

    for diagnostics in by_file.values_mut() {
        diagnostics.sort_by_key(|d| (d.line, d.column));
    }

    by_file
}

/// `path`'s source text for line/col rendering, cached per report pass: `overlays`'
/// content wins over disk when it covers `path` (L1, `docs/PLAN-lsp.md` §3), matching
/// `runner.rs`'s `extract_one` so a diagnostic's span and its rendered line/col are
/// always computed against the same text.
fn read_cached<'a>(
    cache: &'a mut HashMap<PathBuf, String>,
    path: &Path,
    overlays: &Overlays,
) -> &'a str {
    cache.entry(path.to_path_buf()).or_insert_with(|| {
        overlays
            .content(path)
            .map(|content| content.to_string())
            .unwrap_or_else(|| fs::read_to_string(path).unwrap_or_default())
    })
}

/// `--report-unresolved`: emit a warn-severity diagnostic for every checked entry
/// (among `files`) whose specifier failed to resolve (D8's opt-in debug aid). These
/// never affect the exit code (M5 brief §3).
fn collect_unresolved(
    graph: &ModuleGraph,
    files: &[&Path],
    source_cache: &mut HashMap<PathBuf, String>,
    by_file: &mut HashMap<PathBuf, Vec<RenderedDiagnostic>>,
    overlays: &Overlays,
) {
    for &target in files {
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
            let source = read_cached(source_cache, target, overlays);
            let (line, column) = line_col(source, entry.span.start);
            let (end_line, end_column) = line_col(source, entry.span.end);
            by_file
                .entry(target.to_path_buf())
                .or_default()
                .push(RenderedDiagnostic {
                    file: target.to_path_buf(),
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
