//! Node builtin-module check (PLAN-v1.md D5).
//!
//! Must run *before* invoking `oxc_resolver`: `resolve_dts()` has no builtin-module
//! short-circuit of its own — that's only wired into the plain (non-dts) `resolve()`
//! path via `ResolveOptions::builtin_modules` (see docs/research/spike-s3-resolver-provenance.md,
//! gap #2). `nodejs_built_in_modules` is already a transitive dependency of
//! `oxc_resolver`; depending on it directly avoids re-deriving the builtin list.

/// Whether `specifier` names a Node.js builtin module, with or without the `node:`
/// prefix (`"path"` and `"node:path"` both count; modules that *require* the prefix,
/// e.g. `"node:test"`, only count when the prefix is present — `is_nodejs_builtin_module`
/// already encodes that asymmetry).
pub(super) fn is_node_builtin(specifier: &str) -> bool {
    nodejs_built_in_modules::is_nodejs_builtin_module(specifier)
}
