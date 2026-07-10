//! Extension -> `oxc_span::SourceType` mapping, shared by every command that needs
//! to know how to parse a file from its path (`inspect`, the discovery+pipeline
//! runner in [`crate::runner`]).

use std::path::Path;

use oxc_span::SourceType;

/// Extensions recognized by `oxc_span::SourceType::from_path` that ImportLint parses.
/// `.d.ts`/`.d.mts`/`.d.cts` are covered automatically since their final extension is
/// `.ts`/`.mts`/`.cts` — `SourceType` tells them apart from plain `.ts` internally by
/// inspecting the full file name, not just the last extension.
pub const SUPPORTED_EXTENSIONS_MESSAGE: &str =
    "expected one of .js, .mjs, .cjs, .jsx, .ts, .mts, .cts, .tsx, .d.ts, .d.mts, .d.cts";

/// Resolve the `SourceType` to parse `path` with, or `None` if its extension isn't
/// one ImportLint recognizes.
pub fn source_type_for_path(path: &Path) -> Option<SourceType> {
    SourceType::from_path(path).ok()
}
