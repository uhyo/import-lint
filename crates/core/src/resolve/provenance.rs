//! Classify an `oxc_resolver` result as internal project source, external, or
//! unresolved (PLAN-v1.md D5, D8, §2.3). See docs/research/spike-s3-resolver-provenance.md
//! for the evidence behind this exact classification.

use std::path::PathBuf;

use oxc_resolver::{Resolution, ResolveError};

/// Where a resolved specifier's target lives, from the linter's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// Resolved to a project file — subject to the importability check.
    Internal(PathBuf),
    /// Resolved through `node_modules`, a Node builtin, or a self-reference under
    /// `SelfReferenceMode::External` — never checked.
    External,
    /// The resolver failed outright. Skipped silently by default (D8); never a
    /// false positive.
    Unresolved,
}

/// Classify a `resolve_dts()` result exactly as spike S3 tested (14/14 cases,
/// including the workspace-symlink crux case).
///
/// The key signal is `Resolution::package_json()`: it's derived from the resolver's
/// *pre-realpath* `CachedPath` (computed before `symlinks: true` canonicalizes the
/// final `Resolution::path()`), so `package_json().path()` still says
/// `.../node_modules/@ws/pkg/package.json` even when the resolved file itself points
/// at the symlink's real target outside `node_modules`. Do not "simplify" this to a
/// check on `resolution.path()` alone — that's exactly the check that gets symlinked
/// npm/pnpm workspace packages wrong.
pub(super) fn classify(specifier: &str, result: Result<Resolution, ResolveError>) -> Provenance {
    let resolution = match result {
        Ok(r) => r,
        Err(_) => return Provenance::Unresolved,
    };

    let is_bare = !(specifier.starts_with('.') || specifier.starts_with('/'));
    if !is_bare {
        // Relative/absolute specifiers never go through the node_modules walk.
        return Provenance::Internal(resolution.path().to_path_buf());
    }

    let via_node_modules = match resolution.package_json() {
        Some(pkg) => pkg
            .path()
            .components()
            .any(|c| c.as_os_str() == "node_modules"),
        // No package.json found at all (malformed/legacy node_modules package): fall
        // back to the resolved path itself. This only misclassifies the narrow
        // combination of "no package.json" *and* "symlinked outside node_modules" —
        // not detectable through the public API (see spike S3 gap #1).
        None => resolution
            .path()
            .components()
            .any(|c| c.as_os_str() == "node_modules"),
    };
    if via_node_modules {
        Provenance::External
    } else {
        // Bare specifier resolved but not anchored under node_modules: tsconfig
        // `paths`/`baseUrl`, or an ambient module already handled upstream (D5/D6).
        Provenance::Internal(resolution.path().to_path_buf())
    }
}
