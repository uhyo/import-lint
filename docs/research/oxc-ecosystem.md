# Research: oxc Ecosystem for a Rust TS/JS Import-Graph Linter

> Research appendix for [PLAN-v1.md](../PLAN-v1.md). Findings verified against crates.io, docs.rs,
> and the `oxc-project/oxc` / `oxc-project/oxc-resolver` GitHub sources as of 2026-07-10.

## 1. oxc crates — versions, MSRV, stability

| Crate | Version | MSRV | Notes |
|---|---|---|---|
| `oxc_parser` | 0.139.0 | 1.94.0 (edition 2024) | |
| `oxc_ast` | 0.139.0 | 1.94.0 | |
| `oxc_span` | 0.139.0 | 1.94.0 | |
| `oxc_semantic` | 0.139.0 | 1.94.0 | |
| `oxc_diagnostics` | 0.139.0 | 1.94.0 | |
| `oxc_allocator` | 0.139.0 | 1.94.0 | |
| `oxc_jsdoc` | 0.139.0 | 1.94.0 | see §2 |
| `oxc_resolver` | 11.23.0 | 1.88.0 | separate repo/versioning |

All core crates are released in lockstep, roughly weekly (0.130.0 → 0.139.0 over ~9 weeks, mid-May to early-July 2026). They are pre-1.0 and NOT semver-stable — breaking changes land routinely (e.g. `ModuleRecord` moved out of `oxc_semantic` into the parser in a past release, §3). Pin exact versions and re-verify APIs on every upgrade. `oxc_resolver` versions independently (major 11) and moves slower.

## 2. JSDoc support — real, but opt-in and declaration-only

JSDoc lives in the `oxc_jsdoc` crate, wired into `oxc_semantic` behind a Cargo feature:

```toml
# oxc_semantic/Cargo.toml
[features]
default = []
jsdoc = ["dep:oxc_jsdoc"]
linter = ["jsdoc"]   # oxlint itself always pulls this in
```

Enable with `oxc_semantic = { version = "0.139.0", features = ["jsdoc"] }` → `Semantic::jsdoc() -> &JSDocFinder<'a>`.

`JSDocFinder` (`crates/oxc_semantic/src/jsdoc.rs`) attaches comments by matching the span-start of the node:

```rust
pub struct JSDocFinder<'a> {
    attached: FxHashMap<u32, Vec<JSDoc<'a>>>,  // keyed by node span.start
    not_attached: Vec<JSDoc<'a>>,
}
impl<'a> JSDocFinder<'a> {
    pub fn get_one_by_node(&self, nodes: &AstNodes<'a>, node: &AstNode<'a>) -> Option<JSDoc<'a>>; // nearest block
    pub fn get_all_by_node(&self, nodes: &AstNodes<'a>, node: &AstNode<'a>) -> Option<Vec<JSDoc<'a>>>; // farthest..nearest
    pub fn get_all_by_span(&self, span: Span) -> Option<Vec<JSDoc<'a>>>;
    pub fn iter_all(&self) -> impl Iterator<Item = &JSDoc<'a>>;
}
```

`JSDoc<'a>` exposes `.comment()` and `.tags() -> Vec<JSDocTag>`, each tag with `.kind` (e.g. `"package"`) and helpers `.type_name_comment()` / `.type_comment()` / `.comment()`. It is a real structured JSDoc tag parser (multi-line, multibyte, backtick-escaped `@` in code fences — verified against the crate's test suite).

Coverage gaps confirmed from source/issues:
- Only fires on declarations — confirmed working for `function`, `class`, `const`/`let`/`var` (including `export const`), `export default`, `import`, and class property declarations. GitHub issue #1506: class methods and decorated properties in some shapes are not reliably attached — **test `@package` on `interface`/`type` alias before relying on it.**
- Multiple JSDoc blocks before one node all attach, ordered farthest→nearest; `get_one_by_node` returns the nearest.
- GitHub discussion #21507 (April 2026) about missing JSDoc in the napi/JS `oxc-parser` package concerns the Node.js bindings only — the Rust `JSDocFinder` API is real and works.

Recommendation: enable the `jsdoc` feature and use `JSDocFinder` rather than hand-rolling span-based comment association — it already solves "nearest preceding `/**` block while skipping non-JSDoc comments." Fall back to `Semantic::comments()` / `comments_range()` only for node kinds `JSDocFinder` doesn't cover.

## 3. Import/export extraction — `ModuleRecord` (it moved out of `oxc_semantic`)

`ModuleRecord` is NOT on `Semantic`. Breaking changes #7546/#7548 moved it into the parser:

```rust
// oxc_parser::ParserReturn
pub struct ParserReturn<'a> {
    pub program: Program<'a>,
    pub module_record: ModuleRecord<'a>,   // <-- here
    ...
}
```

`ModuleRecord<'a>` (defined in `oxc_syntax::module_record`, arena-backed, ECMA-262-shaped):

```rust
pub struct ModuleRecord<'a> {
    pub has_module_syntax: bool,
    pub requested_modules: ArenaHashMap<'a, Str<'a>, ArenaVec<'a, RequestedModule>>, // specifier -> occurrences
    pub import_entries: ArenaVec<'a, ImportEntry<'a>>,
    pub local_export_entries: ArenaVec<'a, ExportEntry<'a>>,      // export const/function/class
    pub indirect_export_entries: ArenaVec<'a, ExportEntry<'a>>,   // export { x } from './y'
    pub star_export_entries: ArenaVec<'a, ExportEntry<'a>>,       // export * from './y'
    pub exported_bindings: ArenaHashMap<'a, Str<'a>, Span>,
    pub dynamic_imports: ArenaVec<'a, DynamicImport>,             // import('specifier')
    pub import_metas: ArenaVec<'a, Span>,
}
```

`ImportEntry`/`ExportEntry` carry `NameSpan` (name + `Span`) for every part, plus enums `ImportImportName::{Name,NamespaceObject,Default}`, `ExportImportName::{Name,All,AllButDefault,Null}` (`All` = `export * as ns`, `AllButDefault` = bare `export *`), `ExportExportName::{Name,Default,Null}`, `ExportLocalName::{Name,Default,Null}`. `is_type: bool` distinguishes TS `import type`/`export type`. `DynamicImport { span, module_request: Span }` covers `import()`. Complete one-pass extraction — no re-derivation needed.

Cross-file linking is NOT built in ("You must link the module records yourself"). However `oxc_linter` ships a second, owned (non-arena, no lifetime) `ModuleRecord` type purpose-built for cross-file graphs: adds `resolved_absolute_path: PathBuf` and `loaded_modules: RwLock<FxHashMap<CompactStr, Weak<ModuleRecord>>>`, plus `export_default: Option<Span>` and a cached `exported_bindings_from_star_export`. `oxc_linter` isn't published for this use, but read as reference: `crates/oxc_linter/src/module_record.rs`, `module_graph_visitor.rs` (fold-based graph traversal), and `service/runtime.rs` for concurrent project-wide indexing (`papaya::HashMap<Arc<OsStr>, SmallVec<[Arc<ModuleRecord>; 1]>>` — lock-free map chosen over `DashMap` for the hot read path).

## 4. `oxc_resolver` (v11.23.0) — very strong fit

Explicit Rust port of `enhanced-resolve` + `tsconfig-paths-webpack-plugin` + `tsconfck`; used in production by rspack/rolldown/oxlint.

- tsconfig: full `paths` wildcard mapping, `baseUrl`, `extends` chains, project references (`'auto'` or explicit) — the multi-tsconfig monorepo case.
- package.json: `exports`/`imports` with `condition_names` (`"node"`, `"import"`, `"require"`, `"types"`), configurable `exports_fields`.
- `.d.ts` resolution is a first-class separate code path: `dts_resolver.rs` implements `ts.resolveModuleName` with `moduleResolution: "bundler"` — two-pass `node_modules` walk (TS/`.d.ts`+`@types` before `.js`), `@types` scoped-name mangling (`@babel/core` → `@types/babel__core`), `typesVersions`, TS extension substitution (`.js`→`.ts`/`.d.ts`). No hand-rolling needed.
- Symlinks: `symlinks: bool` (default `true`); Yarn PnP out of the box; pnpm via standard symlink following.
- Caching/threading: internal cache (`src/cache/`) backed by `dashmap` + thread-local fast path. Construct one `Resolver` up front and share (`Arc`) across a rayon pool — the cache makes repeated `node_modules` walks cheap.
- Types: `Resolver` (= `ResolverGeneric<FileSystem>`), `ResolverGeneric<Fs: FileSystem>` for custom/virtual filesystems (testing, watch-mode overlay), `ResolveOptions`.

## 5. Performance architecture (oxlint's actual pattern)

Confirmed from `oxlint`/`oxc_linter` sources:

- File walking: the `ignore` crate (with `simd-accel` in the oxlint binary) — same as ripgrep; parallel walking, respects `.gitignore`.
- Parallelism: `rayon`, `--threads` flag wired to `rayon::ThreadPoolBuilder`; defaults to CPU cores. Real-world throughput: ~10,000 files/sec on a 264k-file corpus.
- Allocator pooling: `oxc_allocator` ships `AllocatorPool` (feature `pool`) — each rayon worker gets a reusable bump arena, `reset()` (not dropped) after each file. There's also `FixedSizeAllocatorPool` for large-arena/napi paths.
- **Arena lifetime gotcha**: everything the parser returns (`Program<'a>`, `ModuleRecord<'a>`) borrows the `Allocator`. A cross-file graph outliving per-file parses cannot hold arena-borrowed data. The correct pattern (mirrored by oxlint's lifetime-free `ModuleRecord`): immediately after parse+semantic, copy out only what's needed (specifiers as `String`/`CompactStr`, `Span`s are `Copy`, paths as `PathBuf`) into an owned struct, then reset the arena. Do not thread `'a` through the module graph.
- `indexmap` with the `rayon` feature is used for parallel iteration over ordered maps (deterministic ordering + parallelism).

## 6. Watch mode

- Use **`notify-debouncer-full`** (wraps `notify`): debounces editor event bursts (write, rename-swap, chmod), yields coalesced batches of changed paths per window. Don't embed `watchexec` (a CLI tool, not an embedding API).
- Pitfalls: **WSL2** has a long history of inotify missing/double-firing events for files edited from the Windows side (NTFS-over-9p) — test early; `notify::PollWatcher` (`Config::with_poll_interval`) is the fallback. macOS FSEvents can coalesce rapid edits and deliver directory-level events.
- Incremental re-lint: build the owned module graph with an explicit reverse-edge index (`imported_path -> Vec<importer paths>`). On change: re-parse+re-resolve that file, diff its import/export entries against stored ones, update the graph, transitively invalidate and re-lint affected dependents. Keep last-good diagnostics per file so unaffected files aren't re-emitted.

## 7. Diagnostics / output formats

- `oxc_diagnostics` (0.139.0) depends on **`oxc-miette`** (^3.0.0) — oxc's fork of `miette`, not upstream. Interop with `OxcDiagnostic` requires the fork; standalone use of plain `miette` is fine and better documented.
- ESLint-compatible JSON (confirmed against ESLint formatter docs): array of per-file result objects:

```json
[
  {
    "filePath": "/abs/path/to/file.ts",
    "messages": [
      {
        "ruleId": "import/no-cycle",
        "severity": 2,
        "message": "Circular import detected.",
        "line": 3, "column": 1, "endLine": 3, "endColumn": 24,
        "messageId": "cycle",
        "suggestions": []
      }
    ],
    "errorCount": 1, "warningCount": 0,
    "fixableErrorCount": 0, "fixableWarningCount": 0
  }
]
```

`severity`: 1 = warning, 2 = error. A `json-with-metadata` variant wraps `results` plus `metadata.rulesMeta`.
- GitHub Actions annotations: `::error file={name},line={line},col={col}::{message}` lines to stdout — trivial, no library.
- JUnit XML: small schema; hand-rolled `quick-xml` writer or the `junit-report` crate.

## 8. Config file conventions

Oxlint resolves `.oxlintrc.json` / `.oxlintrc.jsonc` (JSONC support tracked in issue #19729) — JSON-with-comments is the community-expected format, not TOML. Parsing options: `jsonc-parser` (purpose-built: comments + trailing commas, deserializes via serde) or `json5` (full JSON5, more than needed). Recommendation: `jsonc-parser` + `serde`.

## 9. Prior art worth studying

- **Knip** (knip.dev) — closest conceptual match (entry-point-driven graph walk), pure TypeScript; useful as feature/UX reference only.
- **Turbopack** — Rust incremental module graph with watch invalidation; architecturally closest for incremental re-lint, but deeply bundler-specific.
- **rspack/rolldown** — production consumers of `oxc_resolver`; best source of real `ResolveOptions` configuration patterns and resolver edge cases.
- **`oxc_linter` import rules** (`crates/oxc_linter/src/rules/import/{named,export,no_named_as_default}.rs`) and `module_graph_visitor.rs` — a working production example of exactly this problem (graph traversal + cross-file export resolution) on the same primitives. Note their use of `Weak` in `loaded_modules` to avoid `Arc` cycles keeping the whole graph alive when a project shrinks under watch mode.

Sources: crates.io (oxc_parser, oxc_resolver), docs.rs (oxc_semantic), github.com/oxc-project/oxc (`crates/oxc_semantic/src/jsdoc.rs`, `crates/oxc_syntax/src/module_record.rs`, `crates/oxc_linter/src/module_record.rs`, `crates/oxc_allocator/src/pool/`), oxc discussions #21507 / issues #1506, #19729, ESLint formatter docs, notify-rs/notify, knip.dev.
