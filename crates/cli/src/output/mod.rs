//! Output formats for rendered diagnostics (PLAN.md §6, M5): `pretty` (default,
//! ESLint-stylish-like), `json` (ESLint-compatible), `github` (workflow commands).
//!
//! Core's [`import_lint::Diagnostic`] carries no severity or rule id (both are a
//! function of *configuration*, not of the check itself) and `--report-unresolved`
//! diagnostics don't come from core's rule engine at all — [`RenderedDiagnostic`] is
//! the CLI-side type that carries everything a formatter needs, built once in
//! `main.rs` and shared by every formatter.

pub mod eslint_json;
pub mod github;
pub mod pretty;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::ValueEnum;

/// The severity a [`RenderedDiagnostic`] is rendered at. Distinct from
/// [`import_lint::config::Severity`], which also allows `Off` — a rule configured
/// `off` is never checked at all (M5 brief §2), so no diagnostic ever carries that
/// variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSeverity {
    Error,
    Warn,
}

/// One diagnostic ready to render: a core [`import_lint::Diagnostic`] (jsdoc rule
/// violation) or an unresolved-specifier note (`--report-unresolved`), flattened
/// into the same shape with its line/column already computed and its severity and
/// rule id attached.
#[derive(Debug, Clone)]
pub struct RenderedDiagnostic {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub severity: OutputSeverity,
    pub rule_id: &'static str,
    pub message: String,
    pub message_id: String,
}

/// The output format selected by `--format` (PLAN.md §6, M5). Lives here (rather
/// than as a `main.rs`-local `clap` enum) so watch mode (`crates/cli/src/watch.rs`,
/// M6) can render each cycle through the same [`OutputFormat::render`] dispatcher
/// `main.rs`'s one-shot `lint()` uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Pretty,
    Json,
    Github,
}

impl OutputFormat {
    /// Render `diagnostics` in this format to `out`. `cwd` is used by `pretty` and
    /// `github` to display paths relative to it; `linted_files` is used by `json` to
    /// emit an entry for every linted file, even clean ones (ESLint's own
    /// behavior) — unused by the other two formats.
    pub fn render(
        self,
        out: &mut impl Write,
        diagnostics: &[RenderedDiagnostic],
        cwd: &Path,
        colors: bool,
        linted_files: &[PathBuf],
    ) -> io::Result<()> {
        match self {
            OutputFormat::Pretty => pretty::render(out, diagnostics, cwd, colors),
            OutputFormat::Json => eslint_json::render(out, diagnostics, linted_files),
            OutputFormat::Github => github::render(out, diagnostics, cwd),
        }
    }
}
