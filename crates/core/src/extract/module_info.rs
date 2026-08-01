//! Owned, arena-free data model produced by [`crate::extract::extract`].
//!
//! Everything here is `'static`-owned (`CompactStr`/`String`/`PathBuf`/`Span` are all
//! `Copy` or independently heap-allocated) so a `FileModuleInfo` can outlive the oxc
//! arena it was extracted from. See PLAN-v1.md §2.2 for the design rationale.

use std::collections::HashMap;
use std::path::PathBuf;

use oxc_span::Span;
use oxc_str::CompactStr;
use serde::Serialize;

/// Everything ImportLint knows about a single source file after extraction.
#[derive(Debug, Clone, Serialize)]
pub struct FileModuleInfo {
    /// Absolute (or caller-supplied) path to the file this was extracted from.
    pub path: PathBuf,
    /// Import specifiers, default imports, and `export {x} from "y"` specifiers
    /// found in this file — the entries the rule engine will check.
    pub checked_entries: Vec<CheckedEntry>,
    /// What this file offers to importers: exported name -> access info. This is
    /// the "one hop" lookup target for anything that imports from this file.
    pub export_table: HashMap<CompactStr, ExportInfo>,
    /// Specifiers of bare `export * from "..."` statements, in source order.
    pub star_exports: Vec<CompactStr>,
    /// `declare module "x"` names found in this file. Collected unconditionally
    /// during extraction regardless of file extension (D6) — filtering to `.d.ts`
    /// files only is the link phase's responsibility, not extraction's.
    pub ambient_modules: Vec<CompactStr>,
    /// All distinct module specifiers referenced by this file (imports,
    /// re-exports, star exports), for the link phase to resolve.
    pub specifiers: Vec<CompactStr>,
    /// Line ranges silenced by `import-lint-disable-next-line` /
    /// `import-lint-disable-line` directive comments in this file (see
    /// `crate::extract::suppress`). The rule engine drops any diagnostic whose
    /// span starts inside one of these ranges (subject to the rule-name filter).
    pub suppressions: Vec<Suppression>,
}

/// One directive comment's effect: the byte range of the silenced line, plus
/// which rules it silences (empty = every rule).
#[derive(Debug, Clone, Serialize)]
pub struct Suppression {
    /// Byte range of the silenced line in the file's source text (start of line
    /// to its newline, exclusive).
    pub span: Span,
    /// Rule names listed on the directive; empty means the directive applies to
    /// all rules.
    pub rules: Vec<CompactStr>,
}

impl Suppression {
    /// Does this directive silence a diagnostic of rule `rule` whose span starts
    /// at byte `offset`?
    pub fn suppresses(&self, offset: u32, rule: &str) -> bool {
        self.span.start <= offset
            && offset < self.span.end
            && (self.rules.is_empty() || self.rules.iter().any(|r| r == rule))
    }
}

/// The kind of a checkable import/re-export site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// `import { x } from "m"`
    Import,
    /// `import x from "m"` (or `import { default as x } from "m"`)
    ImportDefault,
    /// `export { x } from "m"`
    ReExport,
}

/// One checkable site: an import specifier, a default import, or a re-export
/// specifier with a module source.
#[derive(Debug, Clone, Serialize)]
pub struct CheckedEntry {
    pub kind: EntryKind,
    /// The name being imported/re-exported *at the source module* — e.g. for
    /// `import { x as y } from "m"` this is `x`, not the local alias `y`. For
    /// default imports this is always `"default"`.
    pub imported_name: CompactStr,
    pub specifier: CompactStr,
    /// Span of the whole specifier node, including any `as alias` suffix (D12).
    pub span: Span,
}

/// What a single exported name in this file resolves to.
#[derive(Debug, Clone, Serialize)]
pub struct ExportInfo {
    /// Parsed from the JSDoc on the statement that introduces this export.
    /// `None` means no recognized JSDoc access tag was found (the rule engine
    /// falls back to `defaultImportability` in that case).
    pub access: Option<Access>,
    pub span: Span,
}

/// JSDoc-declared access level (`@public`/`@package`/`@private`, or `@access <level>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Public,
    Package,
    Private,
}
