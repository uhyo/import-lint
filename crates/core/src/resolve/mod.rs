//! Resolve every `(importer, specifier)` pair to a [`Provenance`] (PLAN-v1.md §2.3, M2).
//!
//! This is the link phase's entry point: a single shared [`ProjectResolver`] wraps
//! one `oxc_resolver::Resolver` (its own dashmap-backed cache makes repeated
//! `node_modules`/tsconfig lookups cheap — never construct per-file resolvers, PLAN-v1.md
//! §8), the ambient-module registry built from every extracted file's
//! `FileModuleInfo::ambient_modules` (D6), and a small self-reference package.json
//! cache. `resolve()` is `&self` and safe to call concurrently from rayon workers in
//! the CLI crate.

mod builtins;
mod package_json;
mod provenance;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oxc_resolver::{
    ResolveOptions, Resolver, TsconfigDiscovery, TsconfigOptions, TsconfigReferences,
};
use oxc_str::CompactStr;

pub use provenance::Provenance;

use package_json::PackageJsonCache;

/// How a bare specifier matching the importer's own package name (spec §4.6,
/// `treatSelfReferenceAs`) should be classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelfReferenceMode {
    /// Self-references are external — never checked (the reference plugin's default).
    #[default]
    External,
    /// Self-references fall through to ordinary resolution, which will classify them
    /// as `Internal` (the self-referenced package.json is a project file, not a
    /// `node_modules` one) — checked like any other internal import.
    Internal,
}

/// Wraps the shared `oxc_resolver::Resolver` plus the extra state PLAN-v1.md §2.3's
/// `resolve()` pseudocode needs beyond plain module resolution: the ambient-module
/// registry and the self-reference package.json cache.
pub struct ProjectResolver {
    resolver: Resolver,
    ambient_modules: HashMap<CompactStr, PathBuf>,
    package_json_cache: PackageJsonCache,
    mode: SelfReferenceMode,
}

impl ProjectResolver {
    /// `tsconfig` is the path to the project's `tsconfig.json`, if it has one.
    /// `ambient_modules` maps a `declare module "x"` specifier to the `.d.ts` file
    /// that declares it (built by the caller from every extracted file's
    /// `FileModuleInfo::ambient_modules`, filtered to `.d.ts` files per D6).
    pub fn new(
        project_root: &Path,
        tsconfig: Option<PathBuf>,
        ambient_modules: HashMap<CompactStr, PathBuf>,
        mode: SelfReferenceMode,
    ) -> Self {
        let options = ResolveOptions {
            cwd: Some(project_root.to_path_buf()),
            tsconfig: tsconfig.map(|config_file| {
                TsconfigDiscovery::Manual(TsconfigOptions {
                    config_file,
                    references: TsconfigReferences::Auto,
                })
            }),
            // "types" must be present for `resolve_dts()`'s exports-map lookup to
            // prefer `.d.ts` targets (spike S3 gap #4); import/require/node cover
            // ESM/CJS exports maps encountered along the way.
            condition_names: vec![
                "types".to_string(),
                "import".to_string(),
                "require".to_string(),
                "node".to_string(),
            ],
            main_fields: vec!["module".to_string(), "main".to_string()],
            // TS-style extension substitution: a `.js`/`.jsx` path in source or an
            // exports-map target may actually be implemented by a `.ts`/`.tsx`/`.d.ts`
            // file on disk (e.g. self-reference through `"exports": { ".": "./x.js" }`).
            extension_alias: vec![
                (
                    ".js".to_string(),
                    vec![
                        ".ts".to_string(),
                        ".tsx".to_string(),
                        ".d.ts".to_string(),
                        ".js".to_string(),
                    ],
                ),
                (
                    ".jsx".to_string(),
                    vec![".tsx".to_string(), ".jsx".to_string()],
                ),
                (
                    ".mjs".to_string(),
                    vec![".mts".to_string(), ".d.mts".to_string(), ".mjs".to_string()],
                ),
                (
                    ".cjs".to_string(),
                    vec![".cts".to_string(), ".d.cts".to_string(), ".cjs".to_string()],
                ),
            ],
            extensions: [
                ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".node",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            // Must stay `true`: this is what makes internal-file graph identity
            // (dedup by real path) work. Provenance classification is unaffected by
            // this flag — it comes from `package_json().path()`, which is computed
            // before symlink canonicalization regardless (spike S3).
            symlinks: true,
            ..ResolveOptions::default()
        };
        Self {
            resolver: Resolver::new(options),
            ambient_modules,
            package_json_cache: PackageJsonCache::default(),
            mode,
        }
    }

    /// Resolve one `(importer, specifier)` pair. `importer` is the file doing the
    /// import (not its containing directory — `resolve_dts()` takes a containing
    /// file and derives the directory itself, per spike S3 gap #3).
    pub fn resolve(&self, importer: &Path, specifier: &str) -> Provenance {
        let is_bare = !(specifier.starts_with('.') || specifier.starts_with('/'));

        // 1. Ambient module declarations beat resolution entirely for bare
        // specifiers (D6).
        if is_bare && let Some(declaring_file) = self.ambient_modules.get(specifier) {
            return Provenance::Internal(declaring_file.clone());
        }

        // 2. Node builtins, with or without `node:` (must run before the resolver:
        // `resolve_dts()` has no builtin short-circuit of its own).
        if builtins::is_node_builtin(specifier) {
            return Provenance::External;
        }

        // 3. Self-reference (spec §4.6): only consequential in `External` mode —
        // in `Internal` mode this is a no-op, since falling through to the
        // resolver already classifies the self-referenced package.json as
        // Internal (it's a project file, not a node_modules one), so skip the
        // filesystem walk entirely when it can't change the outcome.
        if is_bare
            && self.mode == SelfReferenceMode::External
            && let Some(dir) = importer.parent()
            && let Some(pkg) = self.package_json_cache.nearest_named(dir)
            && is_self_reference(specifier, &pkg.name)
        {
            return Provenance::External;
        }

        // 4. Fall through to `oxc_resolver`. `resolve_dts()` is the sole correct
        // entry point (spike S3): plain `resolve()` cannot reach types-only
        // packages (no `main`/`module` field at all).
        let result = self.resolver.resolve_dts(importer, specifier);
        provenance::classify(specifier, result)
    }
}

/// `specifier === name || specifier.startsWith(name + "/")`, purely string-based,
/// matching the reference's `lookupPackageJson` comparison exactly (spec §4.6).
fn is_self_reference(specifier: &str, package_name: &str) -> bool {
    specifier == package_name
        || specifier
            .strip_prefix(package_name)
            .is_some_and(|rest| rest.starts_with('/'))
}
