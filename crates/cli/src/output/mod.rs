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

use std::path::PathBuf;

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
