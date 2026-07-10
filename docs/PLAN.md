# ImportLint — Implementation Plan

ImportLint is a standalone Rust CLI linter that reimplements the functionality of
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access)
(v3.1.0) on top of the oxc toolchain, without depending on the TypeScript compiler.

This plan is based on two research documents produced by analyzing the reference
implementation and the oxc ecosystem. **Read both before implementing** — they contain
the precise semantics and API details this plan relies on:

- [`docs/research/eslint-plugin-import-access-spec.md`](./research/eslint-plugin-import-access-spec.md) — the complete behavioral spec of the reference plugin (verified against its implementation and test suite).
- [`docs/research/oxc-ecosystem.md`](./research/oxc-ecosystem.md) — oxc crate versions, APIs, and architecture patterns (verified against sources as of 2026-07-10).

---

## 1. Product decisions (locked)

These decisions resolve the open questions so implementation can proceed without
clarification. Rationale inline.

| # | Decision | Rationale |
|---|---|---|
| D1 | **v1 replicates the reference plugin's behavior exactly**, including its blind spots: `export * from` statements, `import * as ns`, dynamic `import()`, `import x = require()`, and CommonJS `require()` are **not checked**. | Drop-in migration path for existing users; every behavioral divergence is a support burden. Extra strictness can be added later behind new options (see §12). |
| D2 | **One-hop alias resolution**, never transitive. The importability check looks at the export statement in the directly-imported file only; a re-export's own JSDoc (or absence of it) governs, and the re-exporting file is the "exporter" for directory checks. | This is the reference plugin's most consequential semantic (spec §4.3, verified by its tests). Getting this wrong silently diverges on every re-export chain. |
| D3 | The only rule shipped in v1 is **`jsdoc`** (reported as ruleId `import-access/jsdoc` in ESLint-compatible output for drop-in compat). Options and defaults are **identical, name-for-name**, to the reference plugin: `indexLoophole` (true), `filenameLoophole` (false), `defaultImportability` ("public"), `treatSelfReferenceAs` ("external"), `excludeSourcePatterns` ([]), `packageDirectory` (unset). | Migration ergonomics; the config translates 1:1. |
| D4 | **Resolution via `oxc_resolver`** configured from the project's `tsconfig.json` (`paths`, `baseUrl`, `extends`, references) with TS extension substitution and `.d.ts` resolution enabled. We implement one resolution profile (bundler-style + TS extensions, matching `oxc_resolver`'s dts path), not per-`moduleResolution`-mode emulation. | `oxc_resolver` is a production-grade port of enhanced-resolve/tsconfck used by rspack/rolldown/oxlint. Mode-perfect emulation of node16 vs bundler is not worth the cost for a linter: what matters is *which file* a specifier lands on, and the profiles agree in the overwhelming majority of cases. Divergences surface as false "unresolved" (skipped, never a false error — see D8). |
| D5 | **External vs internal classification is resolution-provenance-based**, never a `node_modules` path-substring check: a bare specifier resolved through a `node_modules` directory walk is external *even if symlinks land the real path outside `node_modules`* (npm/pnpm workspaces). Node builtins (`path`, `node:path`) are always external. Bare specifiers matched by tsconfig `paths` or an ambient `declare module` are internal. Self-references follow `treatSelfReferenceAs`. | Required to match the reference (spec §4.6); the workspace-symlink fixtures prove a path check misclassifies. |
| D6 | **Ambient module declarations** (`declare module "x"` in project `.d.ts` files) are collected during the parse phase and take priority over bare-specifier resolution, mapping the specifier to the declaring file as an internal exporter. | Matches reference behavior on the `exclude-patterns` fixtures. |
| D7 | Config file: **`.importlintrc.jsonc`** (also accepts `.importlintrc.json`), parsed with `jsonc-parser` + serde. CLI flags override config. Project root = directory containing the config file (fallback: cwd); all relative-path option matching (`excludeSourcePatterns`, `packageDirectory`) is relative to it. | Follows oxlint convention; JS/TS users expect comments in config. Project root definition replaces `program.getCurrentDirectory()` from the reference. |
| D8 | **Unresolvable imports are skipped silently by default** (matching the reference, where TS would have failed earlier), with an opt-in `--report-unresolved` flag for debugging. | The reference never reports resolution problems; a linter that errors on every resolver divergence would be unusable. |
| D9 | Glob matching (`excludeSourcePatterns`, `packageDirectory`, `include`/`exclude`) uses the **`globset`** crate with `dot: true`-equivalent settings. Minor minimatch/globset divergences are acceptable and caught by the conformance suite. | `globset` is the mature Rust option (ripgrep); writing a minimatch port is not justified. |
| D10 | oxc crates **pinned to an exact version** (0.139.x at time of research; bump deliberately), `oxc_resolver` 11.x. `oxc_semantic` with the `jsdoc` feature; `oxc_allocator` with the `pool` feature. Rust edition 2024, MSRV follows oxc (1.94). | oxc is pre-1.0 with weekly breaking releases; unpinned builds will break. |
| D11 | Cargo **workspace with two crates**: `import_lint` (core library: extraction, graph, rule engine) and `import_lint_cli` (binary `import-lint`: walking, orchestration, output, watch). | The core-as-library split is what later enables the LSP server / VSCode extension without a rewrite. |
| D12 | The reference's `no-program` diagnostic is dropped (no TS program exists here). Everything else in the diagnostic table — messageIds `package`, `package:reexport`, `private`, `private:reexport`, exact message strings, reported identifier = exported name at the hop (e.g. `default`), one diagnostic per specifier, span covering the whole specifier including `as alias` — is replicated exactly. | Output compatibility. |

## 2. Architecture

### 2.1 Pipeline (single run)

```
config load ─► file discovery ─► parse+extract (parallel) ─► link (resolve) ─► check (parallel) ─► report
```

1. **Config load** — locate `.importlintrc.jsonc` upward from cwd (or `--config`); merge CLI flags; locate and parse `tsconfig.json` (path from config, default `./tsconfig.json` if present) for resolver options.
2. **File discovery** — `ignore` crate parallel walker from the configured roots; include `.ts .tsx .mts .cts .js .jsx .mjs .cjs .d.ts .d.mts .d.cts`; respect `.gitignore` plus config `include`/`exclude` globs.
3. **Parse + extract** (rayon, `AllocatorPool`, one arena per worker, `reset()` per file) — per file, produce an **owned** `FileModuleInfo` (see §2.2) and immediately release the arena. This is the *only* phase that touches oxc AST lifetimes.
4. **Link** — resolve every distinct `(importer dir, specifier)` through a single shared `Arc<Resolver>`; consult the ambient-module map first for bare specifiers; classify provenance (Internal(path) / External / Unresolved / SelfReference); build the module graph and the reverse-edge index (needed for watch).
5. **Check** — per file in parallel, for each checkable entry (import specifier, default import, re-export specifier), run the importability algorithm (§3) against the target file's export table. Pure function of `(importer info, exporter info, options)` — no global mutation, trivially parallel.
6. **Report** — sort diagnostics by (file, span), render in the selected format, exit 0 (clean) / 1 (diagnostics) / 2 (usage or internal error).

### 2.2 Core data model (all owned, no arena lifetimes)

```rust
struct FileModuleInfo {
    path: PathBuf,
    // What this file imports/re-exports (the entries we lint):
    checked_entries: Vec<CheckedEntry>,      // import specifiers, default imports, `export {x} from` specifiers
    // What this file offers to importers (the "one hop" lookup target):
    export_table: HashMap<CompactStr, ExportInfo>,  // exported name -> info
    star_exports: Vec<CompactStr>,           // specifiers of `export * from "..."` (order preserved)
    ambient_modules: Vec<CompactStr>,        // `declare module "x"` names found in this file (.d.ts)
    specifiers: Vec<CompactStr>,             // all distinct module specifiers (for the link phase)
}

struct CheckedEntry {
    kind: EntryKind,           // Import | ImportDefault | ReExport
    imported_name: CompactStr, // "default" for default imports
    specifier: CompactStr,
    span: Span,                // whole specifier node incl. `as alias`
}

struct ExportInfo {
    access: Option<Access>,    // parsed from JSDoc on the introducing statement; None -> defaultImportability
    span: Span,
}

enum Access { Public, Package, Private }
```

Extraction detail: `ParserReturn.module_record` provides all entries (`import_entries`,
`local_export_entries`, `indirect_export_entries`, `star_export_entries`); a light AST
visit supplements it with (a) precise full-specifier spans, (b) JSDoc association via
`Semantic::jsdoc()` / `JSDocFinder` on the *statement* node that introduces each export
(declaration statement, `ExportNamedDeclaration`, or `export default`), and (c) ambient
`declare module` names in `.d.ts`. JSDoc→`Access` mapping: scan tags in source order,
first match of `@package` / `@private` / `@public` / `@access <level>` wins (spec §3.1).

The graph is a `HashMap<PathBuf, Arc<FileModuleInfo>>` plus
`resolution: HashMap<(PathBuf, CompactStr), Resolution>` and the reverse index
`importers: HashMap<PathBuf, HashSet<PathBuf>>`. (Start with `std`/`FxHashMap` behind a
`RwLock` or `DashMap`; adopt `papaya` like oxlint only if profiling shows contention.)

### 2.3 The importability check (per `CheckedEntry`)

```
resolve(importer, specifier):
    ambient module match          -> Internal(declaring .d.ts)
    node builtin                  -> External
    self-reference (nearest package.json name matches specifier prefix)
                                  -> External if treatSelfReferenceAs == "external", else resolve as internal
    oxc_resolver:
        via node_modules walk     -> External
        via paths/baseUrl/relative-> Internal(path)
        failure                   -> Unresolved (skip)

check(entry):
    target = resolve(...);  if External or Unresolved -> pass
    if excludeSourcePatterns matches target's project-relative path -> pass
    export_info = lookup(target, entry.imported_name)   // may descend star exports, see below
    access = export_info.access.unwrap_or(defaultImportability)
    Public  -> pass
    Private -> report "private" / "private:reexport"
    Package -> isInPackage(importer, exporter_file) ? pass
             : report "package" / "package:reexport"
```

`lookup`: if the name is in `target.export_table`, done (exporter_file = target — one hop,
even if that entry is itself a passthrough re-export). If not found and `target` has
`star_exports`, descend depth-first through them (cycle-guarded) until a file whose
export table contains the name is found; that file is the exporter. **The exact
star-export semantics must be confirmed against the reference in spike S1 (§10) before
this part is finalized.**

`isInPackage` implements spec §3.3 verbatim: index-loophole suffix strip → package-dir
resolution (walk-up with `packageDirectory` globs, else `dirname`) → same-dir equality →
filename loophole (on raw paths) → descendant-of-exporter-package check.

`self-reference` lookup walks up from the importer to the nearest `package.json`, compares
`specifier == name || specifier.startsWith(name + "/")` (string-based, as the reference).
Cache per-directory `package.json` lookups.

## 3. Directory structure

```
import-lint/
├── Cargo.toml                    # workspace; [workspace.dependencies] pins oxc versions
├── crates/
│   ├── core/                     # crate: import_lint
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs         # RuleOptions, LintConfig, jsonc loading, defaults
│   │       ├── extract/          # parse a file -> FileModuleInfo (only oxc-AST-facing code)
│   │       │   ├── mod.rs
│   │       │   ├── module_info.rs
│   │       │   └── jsdoc.rs      # JSDoc tag -> Access
│   │       ├── resolve/          # Resolver wrapper, provenance classification,
│   │       │   └── ...           # ambient-module registry, package.json cache
│   │       ├── graph.rs          # module graph + reverse index + (watch) invalidation
│   │       ├── rule/
│   │       │   ├── mod.rs        # check() orchestration per entry
│   │       │   ├── in_package.rs # isInPackage, findPackageDirectory, loopholes
│   │       │   └── messages.rs   # messageIds + exact message strings
│   │       └── diagnostics.rs    # Diagnostic {path, span, message_id, identifier, severity}
│   └── cli/                      # crate: import_lint_cli, binary `import-lint`
│       └── src/
│           ├── main.rs           # clap arg parsing
│           ├── walk.rs           # ignore-crate discovery
│           ├── runner.rs         # phases 3–5 orchestration (rayon + AllocatorPool)
│           ├── output/           # pretty.rs, eslint_json.rs, github.rs
│           └── watch.rs          # notify-debouncer-full loop
├── tests/
│   └── conformance/              # ported fixture project + expected-diagnostics snapshots (§9)
├── benches/                      # criterion micro-benches; scripts/bench.sh for hyperfine e2e
└── docs/
    ├── PLAN.md
    └── research/
```

## 4. Configuration file

```jsonc
// .importlintrc.jsonc
{
  "include": ["src"],                 // roots to lint; default: ["."]
  "exclude": ["**/dist/**"],          // in addition to .gitignore
  "tsconfig": "./tsconfig.json",      // for resolver paths/baseUrl; optional
  "rules": {
    "jsdoc": {
      "severity": "error",            // "error" | "warn" | "off"
      // identical to eslint-plugin-import-access, name-for-name:
      "indexLoophole": true,
      "filenameLoophole": false,
      "defaultImportability": "public",
      "treatSelfReferenceAs": "external",
      "excludeSourcePatterns": [],
      "packageDirectory": ["**"]
    }
  }
}
```

Missing config file = all defaults (lint cwd with the rule at `error`). A `rules` map
(not a single option block) keeps the door open for future rules without a config break.

## 5. CLI surface

```
import-lint [paths...]              # override include roots
  --config <path>                   # explicit config file
  --format pretty|json|github       # default pretty; json = ESLint-compatible
  --watch                           # watch mode (§7)
  --watch-poll [interval-ms]        # PollWatcher fallback (WSL2/NFS)
  --threads <n>                     # rayon pool size, default = cores
  --tsconfig <path>
  --report-unresolved               # debug aid (D8)
  --quiet                           # errors only (suppress warns)
```

Exit codes: `0` no findings, `1` findings at error severity, `2` invalid usage/config or internal error.

## 6. Output formats

- **pretty** (default): source-annotated diagnostics with file/line/col, code frame, and the rule name — implemented directly (or via plain `miette`), colors off when not a TTY.
- **json**: ESLint stylish-JSON-compatible array (`filePath`, `messages[{ruleId: "import-access/jsdoc", severity, message, messageId, line, column, endLine, endColumn}]`, `errorCount`, `warningCount`, fixable counts always 0) — enables existing ESLint-output consumers (CI parsers, reviewdog) to work unchanged.
- **github**: `::error file=...,line=...,col=...::message` workflow commands.
- JUnit XML: deferred (post-v1) unless demand appears.

## 7. Watch mode

- `notify-debouncer-full` watching the include roots (plus config/tsconfig files); `--watch-poll` opt-in `PollWatcher` because inotify is unreliable on WSL2 for Windows-side edits — document this prominently.
- Per debounced batch:
  - **Changed file**: re-extract it; diff its `export_table`/`star_exports`/`ambient_modules` against the stored version.
  - **Dirty set** = changed files ∪ (if a file's export surface changed) all files that depend on it. Because of one-hop semantics (D2), a file's diagnostics depend only on: itself, its resolution results, and the export tables of files it directly imports **plus** the transitive closure through `star_exports` chains. The reverse index therefore records both direct-import edges and star-export edges.
  - **Add/delete/rename** (and any `package.json` / `tsconfig.json` / config change): resolution results anywhere may change (a new file can shadow a specifier). Pragmatic v1 policy: clear the resolver cache and re-run the link phase for all files (cheap — resolution is cached and parse results are reused); re-check only files whose resolution results actually changed. Config/tsconfig changes trigger a full reload.
  - Re-check the dirty set; keep last-good diagnostics for untouched files; re-render.
- Interactive niceties: clear screen per cycle, print summary line with timing, debounce window ~50–100 ms.

## 8. Performance strategy

Target: **cold lint of 5,000 files in < 2 s, 10,000 files in < 4 s** on a typical dev
machine (8 cores); watch-mode incremental cycle **< 100 ms** for a single-file edit in a
10k-file project.

- The oxlint recipe (research doc §5): `ignore` parallel walker → rayon workers → `AllocatorPool` arena reuse → extract owned data → arena reset. Never hold arena data across files.
- One shared `Arc<Resolver>`; its dashmap-backed cache makes repeated `node_modules`/tsconfig lookups cheap. Never construct per-file resolvers.
- The check phase is pure computation over small owned structs — negligible next to parse+IO.
- Benchmarks: criterion micro-bench for extraction of a representative file; `hyperfine` end-to-end script against (a) the conformance fixture project scaled up (synthetic generator script producing N-file trees with re-export chains) and (b) a real large repo. CI job tracks the e2e numbers to catch regressions.
- Compare against the ESLint plugin on the same tree for the README's headline number.

## 9. Testing strategy

1. **Conformance suite (the centerpiece).** Port `~/repos/eslint-plugin-import-access/src/__tests__/fixtures/project/` (and the `packages/` workspace fixtures) into `tests/conformance/`. Write a one-off Node script (lives in the reference repo checkout, not shipped) that runs the reference plugin over the fixtures under each tested option set and dumps expected diagnostics as JSON (`file, line, col, endLine, endCol, messageId, identifier`). Rust integration tests run ImportLint over the same tree with the same options and diff against the snapshots. **This is the oracle that removes ambiguity** — every semantic question ("does X error?") is answered by generating the reference's output, not by debate.
2. **Unit tests** for the pure pieces: `isInPackage` (table-driven, every branch of spec §3.3), JSDoc→Access (tag order, `@access` text forms, unrecognized-tag fallthrough), package.json lookup, export-table extraction per declaration form (spec §3.2 table), ambient-module collection.
3. **Resolver integration tests**: tsconfig paths, `exports`-map packages, `types`-only packages, workspace symlink provenance (external), ambient module (internal) — mirroring the reference's `library.ts` matrix.
4. **Watch tests**: harness driving a temp dir (edit/add/delete), asserting diagnostic deltas; use the poll watcher in tests for determinism.
5. **CI**: fmt + clippy + tests on Linux/macOS/Windows (path handling — `path::relative` semantics differ from Node's `path.relative`; Windows separators are a real risk for `isInPackage` string comparisons).

## 10. Pre-implementation spikes (M0)

Small experiments that de-risk the plan; each produces a note in `docs/research/`.

- **S1 — star-export semantics of the reference (behavioral question).** Add temp test fixtures to the reference repo: `import { x } from "./barrel"` where `barrel.ts` is `export * from "./inner"` and `inner.ts` has `@package`/`@private` on `x`. Determine: is the check applied, and is the exporter `inner.ts` (symbol flows through star exports without an alias hop) or skipped? Also `export * as ns from`. Encode the answer in `lookup()` (§2.3) and the conformance snapshots. *This is the one place the spec doc has a genuine gap.*
- **S2 — JSDoc attachment coverage in oxc.** Verify `JSDocFinder` attaches JSDoc to: `export const/function/class`, `export default <expr>`, `ExportNamedDeclaration` (`/** @private */ export { x } from "./y";`), `export interface` / `export type`. Known risk from oxc issue #1506. Fallback for any gap: manual nearest-preceding-`/**`-comment association via `Semantic::comments_range()` (implement behind the same internal API so callers don't care).
- **S3 — resolver provenance.** Confirm `oxc_resolver` exposes enough to classify "resolved through node_modules walk" vs "resolved via paths/relative" (inspect `Resolution` fields / package.json path in the result; worst case: detect whether the *resolution process* consulted a `node_modules` directory by checking the resolved package root). Also verify tsconfig `paths` + `.d.ts` + workspace-symlink cases against the reference's `library.ts` matrix.
- **S4 — one-hop lookup on `export =` / `import =`.** Confirm the reference's behavior for `export =` (ExportAssignment) targets and encode it (likely: treated like default-ish export via `ExportAssignment`; CLI never visits `import x = require()` so only the exporter side matters).
- **S5 — watch on WSL2** (the primary dev environment): confirm `notify` inotify behavior for in-WSL edits; wire `--watch-poll` from day one.

## 11. Milestones and task breakdown

Each milestone ends in a working, testable state.

**M0 — Spikes & scaffolding** (S1–S5 above, plus):
- Workspace setup, pinned deps, CI skeleton (fmt/clippy/test on 3 OSes), `import-lint --version`.
- Conformance oracle script in the reference repo; check generated snapshots into `tests/conformance/expected/`.

**M1 — Extraction**: parse one file → `FileModuleInfo` (all export forms of spec §3.2, JSDoc→Access, ambient modules, precise spans). Unit tests per declaration form. Debug command `import-lint inspect <file>` dumping the extraction as JSON (invaluable for the rest of development).

**M2 — Discovery + link**: `ignore` walker, rayon + `AllocatorPool` pipeline, shared resolver, provenance classification, ambient-module registry, module graph + reverse index. Resolver integration tests (§9.3).

**M3 — Rule engine**: `isInPackage` + loopholes + `packageDirectory` + `excludeSourcePatterns` + self-reference + default options; wire check phase; pretty output. **Exit criterion: full conformance suite green under default options.**

**M4 — Full option matrix**: all non-default option sets from the reference tests (`indexLoophole: false`, `filenameLoophole: true`, `defaultImportability` variants, `treatSelfReferenceAs: "internal"`, `packageDirectory` patterns) green against snapshots.

**M5 — CLI & config polish**: `.importlintrc.jsonc` loading/validation with helpful errors, all flags of §5, `json` + `github` formats, exit codes, README usage docs, migration guide from the ESLint plugin.

**M6 — Watch mode**: §7 design, incremental invalidation, watch tests, WSL2 poll fallback.

**M7 — Performance**: benchmarks (§8), profiling pass, synthetic large-tree generator, README numbers vs the ESLint plugin. Release v0.1.0 (crates.io + prebuilt binaries via GitHub Releases; npm wrapper package can follow).

**M8 (post-v1, good-to-have)**: LSP server crate on top of `import_lint` core (watch-mode graph already provides incremental diagnostics) → VSCode extension; JUnit output; opt-in strict checks for the forms the reference ignores (D1).

## 12. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| oxc pre-1.0 breaking changes | Build breaks on upgrade | Pin exact versions (D10); isolate all oxc-AST-facing code in `extract/` so upgrades touch one module. |
| One-hop semantics implemented subtly wrong | Silent divergence on re-export chains | Conformance oracle (§9.1) covers every reference re-export test; D2 documented in code at the `lookup()` site. |
| `JSDocFinder` gaps on some export forms | Missed annotations | Spike S2 with manual-association fallback. |
| Resolver divergence from TS (`node16` edge cases) | Missed or spurious checks | D8 (skip unresolved, never a false error), `--report-unresolved` for diagnosis; conformance resolver matrix. |
| Provenance classification wrong for symlinked workspaces | False positives on monorepos | Spike S3 + dedicated integration tests mirroring `library.ts`. |
| Windows path semantics (`path.relative` vs Rust) | `isInPackage` wrong on Windows | Normalize to forward-slash internal representation early; Windows CI from M0. |
| WSL2 file watching unreliable | Watch mode broken for the primary user | S5; `--watch-poll` shipped in the same milestone as watch. |
| minimatch vs globset divergence | Option globs behave differently | Acceptable per D9; conformance tests over `packageDirectory`/`excludeSourcePatterns` fixtures pin the observable behavior. |

## 13. Explicitly out of scope for v1

- Checking `export *` statements, namespace member access, dynamic `import()`, `require()` (D1 — future opt-in).
- Auto-fixes / suggestions (the reference has none either).
- TS Language Service plugin parity (auto-import filtering) — superseded by the future LSP (M8).
- Per-glob option overrides (the reference doesn't have them; revisit with real user demand).
