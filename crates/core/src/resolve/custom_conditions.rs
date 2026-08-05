//! Read `compilerOptions.customConditions` out of a tsconfig `extends` chain.
//!
//! `oxc_resolver` applies the tsconfig's `paths`/`baseUrl` itself but does not
//! parse `customConditions`, so [`ProjectResolver::new`](super::ProjectResolver::new)
//! reads it here and appends the result to `ResolveOptions::condition_names` —
//! that one list is what both `resolve()` and `resolve_dts()` consult for
//! `exports`-map and `#`-subpath-`imports` condition matching.
//!
//! Semantics mirror TypeScript's config merging: `customConditions` is a single
//! (non-merged) compiler option, so the nearest definition wins — the config
//! itself first, then its `extends` targets with later array entries taking
//! precedence over earlier ones. Lookup is deliberately lenient: an unreadable
//! or malformed config contributes nothing here, and the resolver's own tsconfig
//! loading surfaces the error.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use jsonc_parser::ParseOptions;
use oxc_resolver::{ResolveOptions, Resolver};
use serde::Deserialize;

/// The two tsconfig fields the lookup cares about; everything else is ignored.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawTsconfig {
    extends: Option<Extends>,
    compiler_options: RawCompilerOptions,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawCompilerOptions {
    custom_conditions: Option<Vec<String>>,
}

/// `extends` accepts a single specifier or (TS 5.0+) an array of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum Extends {
    One(String),
    Many(Vec<String>),
}

impl Extends {
    fn specifiers(&self) -> &[String] {
        match self {
            Extends::One(specifier) => std::slice::from_ref(specifier),
            Extends::Many(specifiers) => specifiers,
        }
    }
}

/// The effective `customConditions` for the config at `tsconfig` (empty when the
/// chain never sets it).
pub(super) fn custom_conditions(tsconfig: &Path) -> Vec<String> {
    Walker::default()
        .lookup(tsconfig.to_path_buf())
        .unwrap_or_default()
}

#[derive(Default)]
struct Walker {
    /// Circular-`extends` guard.
    visited: HashSet<PathBuf>,
    /// Resolver for `extends` targets that go through `node_modules` (bare
    /// package specifiers and `#` subpath imports), built only if the chain
    /// actually contains one. Options mirror `oxc_resolver`'s own tsconfig-
    /// extends lookup so both agree on which file a specifier means.
    extends_resolver: Option<Resolver>,
}

impl Walker {
    fn lookup(&mut self, path: PathBuf) -> Option<Vec<String>> {
        let path = if path.is_dir() {
            path.join("tsconfig.json")
        } else {
            path
        };
        if !self.visited.insert(path.clone()) {
            return None;
        }
        let text = fs::read_to_string(&path).ok()?;
        let raw: RawTsconfig =
            jsonc_parser::parse_to_serde_value(&text, &ParseOptions::default()).ok()?;
        if raw.compiler_options.custom_conditions.is_some() {
            return raw.compiler_options.custom_conditions;
        }
        let extends = raw.extends?;
        let directory = path.parent()?.to_path_buf();
        // Later `extends` entries take precedence over earlier ones (matching
        // TypeScript), so search back-to-front and stop at the first hit.
        extends.specifiers().iter().rev().find_map(|specifier| {
            let target = self.resolve_extends(&directory, specifier)?;
            self.lookup(target)
        })
    }

    /// Resolve one `extends` specifier to a config file path, mirroring
    /// TypeScript: rooted and `./`-relative paths get a `.json`-appending
    /// fallback; everything else (bare packages, `#` imports) resolves through
    /// `node_modules`.
    fn resolve_extends(&mut self, directory: &Path, specifier: &str) -> Option<PathBuf> {
        if Path::new(specifier).is_absolute() {
            return existing_config(PathBuf::from(specifier));
        }
        if specifier.starts_with('.') {
            return existing_config(directory.join(specifier));
        }
        let resolver = self.extends_resolver.get_or_insert_with(|| {
            Resolver::new(ResolveOptions {
                condition_names: vec!["node".to_string(), "import".to_string()],
                extensions: vec![".json".to_string()],
                main_files: vec!["tsconfig".to_string()],
                ..ResolveOptions::default()
            })
        });
        resolver
            .resolve(directory, specifier)
            .ok()
            .map(|resolution| resolution.into_path_buf())
    }
}

/// `path` if it exists, else `path` with `.json` appended (TypeScript's fallback
/// for a relative/rooted `extends` written without the extension).
fn existing_config(path: PathBuf) -> Option<PathBuf> {
    if path.exists() {
        return Some(path);
    }
    if path.extension().is_some_and(|ext| ext == "json") {
        return None;
    }
    let mut with_json = path.into_os_string();
    with_json.push(".json");
    let with_json = PathBuf::from(with_json);
    with_json.exists().then_some(with_json)
}
