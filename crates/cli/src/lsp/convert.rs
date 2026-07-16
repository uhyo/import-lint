//! URI<->path and diagnostic conversion for the LSP server (L2, `docs/PLAN-lsp.md` §2).
//!
//! Path identity matters here: every path this module hands back must be in the same
//! form the watch engine's module graph keys files by — a canonicalized path, per
//! `WatchSession::set_overlay`'s doc comment and `crates/cli/src/walk.rs`'s "returned
//! paths are canonicalized" contract. [`uri_to_canonical_path`] does that conversion;
//! [`path_to_uri`] is its inverse for outgoing `publishDiagnostics`.
//!
//! Windows note: `url::Url::from_file_path` (verified against the `url` 2.5.8 source,
//! `path_to_file_url_segments_windows`) treats a `\\?\`-verbatim disk prefix
//! (`Prefix::VerbatimDisk`) identically to a plain drive prefix (`Prefix::Disk`) —
//! both produce a normal `file:///C:/...` URI with no verbatim segments. So a
//! canonicalized (verbatim-prefixed, on Windows) engine path converts to a clean URI
//! with no extra stripping step needed (decision D-L7 anticipated needing the `dunce`
//! crate for this; it turned out not to be necessary — see the L2 report).

use std::path::{Path, PathBuf};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::output::{OutputSeverity, RenderedDiagnostic};

/// Resolve a `file:` URI to the canonical path form the module graph uses, or `None`
/// if the URI isn't a `file:` URI, doesn't exist on disk (untitled buffers, deleted
/// files), or otherwise fails to canonicalize.
pub fn uri_to_canonical_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok()?.canonicalize().ok()
}

/// The inverse of [`uri_to_canonical_path`] for outgoing notifications: `None` if
/// `path` can't be turned into a `file:` URI (e.g. it isn't absolute).
pub fn path_to_uri(path: &Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

/// Convert one engine diagnostic to its LSP form (decision E5: UTF-16 positions,
/// 1-based -> 0-based is the only transform needed since `RenderedDiagnostic`'s
/// columns are already UTF-16 code units, matching LSP's default encoding).
pub fn to_lsp_diagnostic(diagnostic: &RenderedDiagnostic) -> Diagnostic {
    Diagnostic {
        range: Range::new(
            Position::new(diagnostic.line - 1, diagnostic.column - 1),
            Position::new(diagnostic.end_line - 1, diagnostic.end_column - 1),
        ),
        severity: Some(match diagnostic.severity {
            OutputSeverity::Error => DiagnosticSeverity::ERROR,
            OutputSeverity::Warn => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diagnostic.rule_id.to_string())),
        code_description: None,
        source: Some("import-lint".to_string()),
        message: diagnostic.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}
