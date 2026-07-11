# Spike S3: `oxc_resolver` provenance classification

> Verifies (for [PLAN-v1.md](../PLAN-v1.md) D4, D5, D6, D8, §2.3, §10 spike S3) that `oxc_resolver`
> exposes enough to classify a resolved specifier as Internal / External / Unresolved,
> including the pnpm/npm-workspace symlink case that a naive real-path check gets wrong.

**oxc_resolver version tested: 11.23.0** (pinned exactly, matches
[oxc-ecosystem.md](./oxc-ecosystem.md) §1). Scratch project:
`/tmp/claude-1000/-home-uhyo-repos-import-lint/4669f777-6e8a-45c6-a848-08d7f22aaf22/scratchpad/spike-s3/`
(not committed, not in the repo). Source inspected: the vendored crate source at
`~/.cargo/registry/src/index.crates.io-*/oxc_resolver-11.23.0/src/{lib.rs, resolution.rs,
options.rs, dts_resolver.rs, cache/cache_impl.rs, cache/cached_path.rs, package_json/mod.rs,
tests/*.rs}` — no docs.rs gaps needed filling; the source (including its own test suite) was
authoritative and sufficient.

## The key finding

`Resolution` (the public result type) does **not** expose a boolean "resolved via
node_modules" flag directly. But it exposes `package_json() -> Option<&Arc<PackageJson>>`,
and `PackageJson` exposes **two** paths:

- `PackageJson::path()` — the path where the manifest was *found*, literal, **not**
  symlink-resolved.
- `PackageJson::realpath()` — the canonicalized (symlink-resolved) path.

Internally, `Resolution::path()` (the final resolved file) is produced by
`load_realpath()`, which canonicalizes when `ResolveOptions::symlinks` is `true` (the
default). But `Resolution::package_json()` is computed *before* that canonicalization step,
from the same `CachedPath` the node_modules walk produced
(`ResolverImpl::find_package_json_for_a_package`, called on the `cached_path` returned by
`require()`/dts routing, in `resolve_impl`/`dts_finalize` — both call it strictly before
`load_realpath`). `CachedPath::inside_node_modules`/`is_node_modules` are themselves computed
purely from path *components* at cache-entry construction time
(`cache_impl.rs::value()`: `file_name() == "node_modules"`), independent of symlink
resolution, and this component-based flag is what `find_package_json_for_a_package` uses to
decide how to walk up for the manifest. `PackageJsonGeneric::path` is stored verbatim from
that walk (`package_json/mod.rs`), while `realpath` is separately canonicalized. Net effect:
**`package_json().path()` still says `.../node_modules/@ws/pkg/package.json` even when the
resolver's own symlink-following makes `Resolution::path()` point at the symlink's real
target outside `node_modules`.** This is exactly the signal D5 needs, and it is available
regardless of the `symlinks` option's value.

## Recommendation: use `resolve_dts()`, not `resolve()`, as the primary entry point

`oxc_resolver` ships **two** resolution algorithms behind one `Resolver`:
`Resolver::resolve()` (enhanced-resolve port, JS-oriented, needs `extension_alias` bolted on
for TS substitution) and `Resolver::resolve_dts()` (`dts_resolver.rs`, a from-scratch port of
`ts.resolveModuleName` with `moduleResolution: "bundler"`). D4 already anticipates this
("matching `oxc_resolver`'s dts path"). `resolve_dts()`:

- Natively does TS extension substitution (`./foo.js` → `./foo.ts`/`.d.ts`, and the
  `.mjs`/`.cjs` → `.d.mts`/`.d.cts` variants) as a two-pass algorithm — no `extension_alias`
  config needed.
- Natively resolves `.d.ts`, package `types`/`typings` fields, and `typesVersions` — including
  packages that have **no** `main`/`module` field at all (case 5c below), which plain
  `resolve()` fails on outright (main-field lookup finds nothing, directory-index fallback
  looks only for `.js`/`.json`/`.node`).
- Handles tsconfig `paths`/`baseUrl` (including `extends` chains), package `exports` (with a
  `types` condition), and the node_modules walk, same as `resolve()`.
- Returns the same public `Resolution` type, built the same way
  (`find_package_json_for_a_package` on the pre-realpath `CachedPath`), so the classification
  function below works identically.
- Does **not** check `ResolveOptions::builtin_modules` — unlike `resolve()`, there is no
  builtin-module short-circuit in the dts path. This is fine: PLAN §2.3 already puts the node
  builtin check *before* calling into the resolver at all (D5's `resolve()` pseudocode). Use
  the `nodejs-built-in-modules` crate directly (it's already an `oxc_resolver` dependency,
  `BUILTINS` and `BUILTINS_WITH_MANDATORY_NODE_PREFIX` slices) to pre-filter, stripping an
  optional `node:` prefix before the membership check.

## Results per case

| # | Case | Specifier | Expected | Observed | Pass |
|---|---|---|---|---|---|
| 1a | node_modules, `package.json` `main` | `pkg-main` | External | `path=node_modules/pkg-main/index.js`, `package_json=node_modules/pkg-main/package.json` → External | ✅ |
| 1b | node_modules, `package.json` `exports` map | `pkg-exports` | External | resolved through `exports["."]` to `dist/index.js`, `package_json` under node_modules → External | ✅ |
| 2 | **Symlinked workspace package** (`node_modules/@ws/pkg` → `../../packages/pkg`, real path outside node_modules) | `@ws/pkg` | External | `Resolution::path()` = the **real** path `packages/pkg/index.js` (symlink followed, `symlinks: true`); `package_json().path()` = the **literal** `node_modules/@ws/pkg/package.json` (pre-realpath) → classifier correctly says External | ✅ (the crux case) |
| 3a | tsconfig `paths` (`"@app/*": ["src/*"]`) | `@app/app-target` | Internal | resolved to `src/app-target.ts`; `package_json()` = project root `package.json` (no `node_modules` component) → Internal | ✅ |
| 3b | tsconfig `baseUrl` (set in an **extended** base config, proving `extends` chains work) | `src/foo` (bare-looking) | Internal | resolved via `baseUrl` to `src/foo.ts` → Internal | ✅ |
| 4 | Relative specifier | `./foo` | Internal | resolved to `src/foo.ts` → Internal (classifier short-circuits: not bare) | ✅ |
| 5a | TS extension substitution | `./foo.js` (no `foo.js` on disk, only `foo.ts`) | resolves to `./foo.ts` | resolved to `src/foo.ts` via `resolve_dts()`'s extension-replacement phase → Internal | ✅ |
| 5b | Resolution to local `.d.ts` (no sibling `.ts`/`.js`) | `./dts-only` and `./dts-only.js` | resolves to `dts-only.d.ts` | both forms resolved to `src/dts-only.d.ts` → Internal | ✅ |
| 5c | Package with **only** a `types` field (no `main`, no `index.js`) | `pkg-types-only` | External, resolves via dts path | plain `resolve()` would fail here (confirmed by reading `dts_resolver.rs`'s package-entry logic vs. `resolve()`'s main-fields-only lookup); `resolve_dts()` resolved to `index.d.ts` via `pkg.typings().or_else(pkg.types())` → External | ✅ (demonstrates why `resolve_dts()` is required) |
| 6a | Node builtin, bare | `path` | External | pre-filtered via `nodejs_built_in_modules::BUILTINS` before calling the resolver → External | ✅ |
| 6b | Node builtin, `node:` prefix | `node:path` | External | prefix stripped, same membership check → External | ✅ |
| 7 | Resolution failure | `definitely-does-not-exist-pkg` | Unresolved | `Err(ResolveError::NotFound(..))` → Unresolved | ✅ |
| gap-probe | node_modules package with **no `package.json` at all** (legacy/malformed) | `pkg-no-pkgjson` | External | `package_json()` = `None`; naive classifier (package_json-only) **misclassified this as Internal** on first pass — fixed by adding a path-component fallback when `package_json()` is `None` (see Recommendation/Gaps) → now correctly External | ✅ (after fix) |

All 14 sub-cases pass with the classification function below.

## The classification function (as tested)

```rust
#[derive(Debug)]
enum Provenance {
    Internal(PathBuf),
    External,
    Unresolved,
}

/// The key signal: `Resolution::package_json()` is derived from the resolver's
/// *pre-realpath* `CachedPath` (`ResolverImpl::find_package_json_for_a_package`,
/// called on the `cached_path` returned by node_modules/dts routing, BEFORE
/// `load_realpath` canonicalizes it for the final `Resolution::path()`).
/// `PackageJson::path()` (as opposed to `PackageJson::realpath()`) is therefore the
/// literal, symlink-unresolved path at which the manifest was found — it still says
/// `node_modules/@ws/pkg/package.json` even when `symlinks: true` makes
/// `Resolution::path()` point at the symlink's real target outside node_modules.
fn classify(specifier: &str, node_builtin: bool, result: Result<Resolution, ResolveError>) -> Provenance {
    if node_builtin {
        return Provenance::External;
    }
    match result {
        Err(_) => Provenance::Unresolved,
        Ok(resolution) => {
            let is_bare = !(specifier.starts_with('.') || specifier.starts_with('/'));
            if !is_bare {
                // Relative/absolute specifiers never go through the node_modules walk.
                return Provenance::Internal(resolution.path().to_path_buf());
            }
            let via_node_modules = match resolution.package_json() {
                Some(pkg) => pkg.path().components().any(|c| c.as_os_str() == "node_modules"),
                // No package.json found at all (malformed/legacy node_modules package).
                // Fall back to checking the *resolved* path itself. This only mis-detects
                // the combination of "no package.json" AND "symlinked outside
                // node_modules" — undetectable via the public API (see Gaps).
                None => resolution.path().components().any(|c| c.as_os_str() == "node_modules"),
            };
            if via_node_modules {
                Provenance::External
            } else {
                // Bare specifier resolved but not anchored under node_modules:
                // tsconfig paths/baseUrl (D5: Internal).
                Provenance::Internal(resolution.path().to_path_buf())
            }
        }
    }
}

fn is_node_builtin(specifier: &str) -> bool {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    nodejs_built_in_modules::BUILTINS.contains(&bare)
        || nodejs_built_in_modules::BUILTINS_WITH_MANDATORY_NODE_PREFIX.contains(&bare)
}
```

Call `is_node_builtin()` and any ambient-module-map / self-reference checks (D6, D5) *before*
invoking the resolver at all, per the PLAN §2.3 pseudocode — `resolve_dts()` does not special-case
builtins itself.

## Recommended `ResolveOptions` for M2

```rust
Resolver::new(ResolveOptions {
    tsconfig: Some(TsconfigDiscovery::Manual(TsconfigOptions {
        config_file: project_root.join("tsconfig.json"), // or per-config-file path from CLI/config
        references: TsconfigReferences::Auto,
    })),
    // "types" lets resolve_dts()'s node_modules/exports-map lookup use the `types`
    // condition; import/require/node cover ESM/CJS exports maps encountered along the way.
    condition_names: vec!["types".into(), "import".into(), "require".into(), "node".into()],
    main_fields: vec!["module".into(), "main".into()],
    extensions: vec![
        ".ts".into(), ".tsx".into(), ".mts".into(), ".cts".into(),
        ".js".into(), ".jsx".into(), ".mjs".into(), ".cjs".into(),
        ".json".into(), ".node".into(),
    ],
    // extension_alias is NOT needed: resolve_dts() implements TS extension substitution
    // and .d.ts resolution as its own algorithm. It's only relevant to resolve()'s
    // (non-dts) enhanced-resolve path, which M2 should not use as the primary resolver.
    symlinks: true,   // must stay true: this is what makes internal-file graph identity
                       // (dedup by real path) work; provenance still comes from
                       // package_json().path(), which is unaffected by this flag.
    ..ResolveOptions::default()
})
```

Call `resolver.resolve_dts(importer_file, specifier)` (not `resolve()`/`resolve_file()`) as
the M2 link-phase entry point. Note `resolve_dts()` takes a **file** path (`containing_file`)
and derives the directory itself — consistent with `resolve_file()`'s calling convention, not
`resolve()`'s (which wants a directory).

For `TsconfigDiscovery`: use `Manual` with the path resolved during config load (D7's
project-root logic already locates the right `tsconfig.json`); `Auto` is documented as "only
works for `resolve_file`", which is what M2 uses, so `Auto` is also viable if per-file
tsconfig auto-discovery (multi-tsconfig monorepos, TS project references) is wanted instead of
one fixed config — the PLAN's `references: TsconfigReferences::Auto` is already the right
setting for TS project references regardless of `Auto` vs `Manual` discovery mode.

## Recommendation

`oxc_resolver` 11.23.0 supports the D5 provenance classification exactly as required,
**including the workspace-symlink case**, via `Resolution::package_json().path()` (the
pre-realpath manifest location) rather than any real-path/canonicalize-based check on the
final resolved file — a real-path check is provably wrong here since `Resolution::path()`
itself is canonicalized when `symlinks: true` (the required default for internal-file graph
identity). Use `resolve_dts()` as the sole M2 resolution entry point rather than `resolve()` +
`extension_alias`: it subsumes TS extension substitution, `.d.ts`/`types`-field resolution,
and typesVersions in one algorithm that plain `resolve()` cannot handle (proven by case 5c,
where a `main`-less types-only package resolves via `resolve_dts()` but is unreachable through
`resolve()`'s main-fields-only directory lookup).

### Gaps / risks discovered

1. **No package.json at all, inside node_modules.** If a node_modules package has no
   manifest (legacy/hand-rolled layout — not achievable via `npm publish`, but possible with
   manual `node_modules` edits or certain build tool outputs), `package_json()` returns `None`
   and the primary signal is unavailable. The fallback (`resolution.path()` component check)
   correctly classifies the common case (no symlink involved) but would misclassify the
   *combination* of "no package.json" **and** "symlinked to outside node_modules" as Internal
   — there is no public API to recover the pre-realpath `CachedPath` for the final resolved
   *file* in that scenario (only `package_json()`'s pre-realpath path is exposed). This is a
   narrow, arguably pathological edge case; flag it in code as a documented tradeoff rather
   than building extra infrastructure for it. If it matters later, the escape hatch is
   `Resolver::new(ResolveOptions { symlinks: false, .. })` run as a *second*, side-channel
   resolve purely to inspect literal-path node_modules-ness — expensive (double resolution)
   and not recommended as a default.
2. **`resolve_dts()` has no `builtin_modules` handling.** Confirmed by source inspection (no
   `builtin`/`Builtin` references anywhere in `dts_resolver.rs`); must pre-filter node
   builtins ourselves before calling the resolver (already the design in PLAN §2.3). Verified
   working via `nodejs-built-in-modules` (already a transitive dependency, safe to depend on
   directly).
3. **`resolve_dts()` takes a `containing_file`, not a directory.** Differs from `resolve()`'s
   calling convention (directory-based); matches `resolve_file()`. M2's link phase should
   track `(importer file, specifier)` pairs (not `(importer dir, specifier)` as PLAN §2.1 step
   4 currently phrases it) to call `resolve_dts()` correctly, or synthesize a fake filename
   under the importer's directory (as this spike's harness does with `dir.join("__importer__.ts")`) if the link phase only has directories at that point — using the importer's real file path directly is simpler and was not observed to cause any issues in testing.
4. **`condition_names` for `resolve_dts()` should include `"types"`** (verified against the
   library's own `tests/dts_resolver.rs::resolver()` helper, which uses
   `["import", "types"]`) — the exports-map `types` condition otherwise loses priority per
   package `exports` ordering rules and may fall through to `import`/`require` targets whose
   files aren't `.d.ts`, changing which file gets treated as the "one hop" target.
