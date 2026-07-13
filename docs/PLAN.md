# ImportLint — LSP Server & VS Code Extension Plan (M8)

ImportLint ships as a CLI (crates.io `import-lint`, npm `@import-lint/cli` + six
platform packages, GitHub Release binaries). The CLI's watch mode already delivers
~5ms incremental re-check cycles on 10k-file trees — but users see violations only
in a terminal. This plan puts those diagnostics in the editor: an LSP server built
into the existing binary, and a VS Code extension as the first (and reference)
client.

Earlier plans are archived at [`docs/PLAN-v1.md`](./PLAN-v1.md) (core linter,
M0–M7) and [`docs/PLAN-npm.md`](./PLAN-npm.md) (npm distribution, N1–N3).

**Goal:** open a TypeScript project with an `.importlintrc.jsonc`, see
import-access violations as you type — including in files you *didn't* edit
(cross-file invalidation is the whole point of this linter) — with zero extra
installs beyond `npm install -D @import-lint/cli` and the extension itself.

---

## 1. Product decisions (locked)

| # | Decision | Rationale |
|---|---|---|
| E1 | The LSP server ships **inside the existing `import-lint` binary** as a subcommand: **`import-lint lsp`** (stdio transport). No new crate, no new binary, no new packages. | Ruff's `ruff server` model. Reuses the entire crates.io/npm/GH-Release distribution as-is; the server version is automatically the project's linter version. Technically forced, too: the watch engine (`WatchSession`, `ExtractionCache`) lives in `crates/cli` behind `pub(crate)` — an in-crate `lsp/` module needs no visibility bumps or crate reorg. |
| E2 | LSP library: **`lsp-server` + `lsp-types`** (rust-analyzer's stack), synchronous stdio main loop with `crossbeam_channel::select!`. | Our engine is synchronous (rayon inside); `lsp-server` is the only mainstream option with a plain sync API and is what Ruff/ty (same architecture) chose. No tokio dependency in the shipped binary. Known cost: `lsp-types` is stale-but-functional; we need nothing newer than LSP 3.16 features. Pinned to `=0.95.1` (decided during L2): 0.96+ replaced `url::Url` with a fluent-uri `Uri` that has no `to_file_path()`, and hand-rolling file-URI↔path conversion is a Windows edge-case minefield for zero feature gain. Fallback if we ever need async: `tower-lsp-server` (the maintained community fork used by oxc/Biome). |
| E3 | Diagnostics are **push-only** (`textDocument/publishDiagnostics`) and published for **all project files, not just open documents**. After every engine cycle, the server diffs the full diagnostic set against what it last published per file and (re)publishes changed files, sending empty arrays to clear. | The engine computes the whole project's diagnostics every cycle anyway (`run_cycle` returns the full set) — publish-all is free, and it's the product: editing `a.ts` can create a violation in closed `b.ts`, and the user must see it in the Problems panel. Biome republishes open docs only because its engine is per-file on-demand; ours isn't. Pull diagnostics (`textDocument/diagnostic`) adds capability negotiation for no benefit here. |
| E4 | **Lint-as-you-type via buffer overlays**: every open text document's in-memory content overrides its on-disk content in extraction *and* in diagnostic line/col rendering. `didChange` is debounced (~200ms) and mapped to the existing incremental fast path. A **`importLint.run`: `"onType"` (default) \| `"onSave"`** setting (oxc precedent) lets users opt out of keystroke linting. | The seams already exist: core `extract()` takes `source_text: &str` (never touches disk), and `WatchSession::run_cycle(&[ChangeKind])` was designed notify-free. The overlay must cover **all** open documents, not just the one that changed — otherwise a file importing another unsaved buffer resolves against stale disk content. |
| E5 | Position encoding: **UTF-16 only** in v1 (the LSP default; no `positionEncoding` negotiation). | `line_col()` already emits UTF-16 code-unit columns (ESLint convention) — LSP conversion is literally subtracting 1 from line and column. Negotiating UTF-8 à la Biome is a later micro-optimization. |
| E6 | VS Code extension: **one universal `.vsix`, no bundled binary, no downloads**. Binary resolution order: (1) `importLint.binaryPath` setting; (2) workspace `node_modules` — resolve `@import-lint/cli/package.json`, then the `@import-lint/<platform-key>` binary package (same key computation as the npm shim, incl. musl detection); (3) `PATH`. If nothing is found, show one actionable notification (install instructions), don't error-loop. | The Biome/oxc locator model, and the payoff of the npm milestone: projects that installed `@import-lint/cli` get a version-matched server with zero config. Bundling binaries (Ruff model) would mean 6+ platform-specific vsix builds per release for no gain. |
| E7 | The extension lives **in-repo at `editors/vscode/`** (TypeScript, `vscode-languageclient`, esbuild bundle) and is released **independently of the linter** via **`vscode-v*` tags** publishing to **VS Code Marketplace + Open VSX** from CI. | Monorepo keeps extension and server protocol changes in one PR; oxc/Biome/Ruff use separate repos but we don't have their scale. The extension is a thin client that runs whatever binary the workspace has — its version line is decoupled from linter releases by design, so lockstep `v*` tagging would force empty releases. Open VSX covers Cursor/VSCodium/Windsurf users. |
| E8 | Config/tsconfig changes: the server registers `workspace/didChangeWatchedFiles` (dynamic registration) for `.importlintrc.json{,c}` and the configured tsconfig, mapping them to the existing `ChangeKind::ConfigChanged`/`TsconfigChanged` hot-reload path. Escape hatch: **`importLint.restart`** command. | Watch mode's config hot-reload (re-load, full re-check, non-fatal on parse error) comes for free through `run_cycle`; the LSP client's file watcher replaces `notify`, which also sidesteps the WSL2 watcher problems entirely. |
| E9 | The extension **starts the server only when a config file is present** in the workspace (`importLint.enabled`: `"auto"` (default) \| `"on"` \| `"off"`). | Activating in every JS/TS project and linting with implicit defaults would surprise users who never opted in. `"on"` covers zero-config users who want defaults. |
| E10 | v1 scope is **diagnostics only**: no code actions/quick fixes, no hover, single workspace folder (first root wins; log a warning for multi-root). | Ship the vertical slice. Quick fixes (e.g. "annotate with `@public`") and multi-root need design of their own and nothing in v1 forecloses them. |

## 2. Server architecture (`crates/cli/src/lsp/`)

```
import-lint lsp            # new clap subcommand, stdio only
└── main loop (lsp_server::Connection::stdio + crossbeam select!)
    ├── initialize: capabilities = { textDocumentSync: FULL, ... }, serverInfo
    ├── initialized: register didChangeWatchedFiles for config/tsconfig
    ├── didOpen(uri, text)    → set_overlay(path, text) + schedule cycle
    ├── didChange(uri, text)  → set_overlay(path, text) + debounce → cycle
    ├── didSave(uri)          → cycle now (flushes debounce)
    ├── didClose(uri)         → clear_overlay(path) + cycle (disk is truth again)
    ├── didChangeWatchedFiles → ChangeKind::{ConfigChanged,TsconfigChanged,Structural}
    └── cycle = WatchSession::run_cycle(&changes)
                → diff full diagnostic set vs published_map
                → publishDiagnostics per changed file (empty array to clear)
```

Notes:

- **Text sync is FULL**, not incremental — the engine re-extracts a changed file
  wholesale anyway (~47µs), so incremental sync would only save protocol bytes.
- **Debounce** is a `select!` timeout arm, no extra thread: pending `ContentEdit`s
  accumulate and flush after 200ms of quiet (or immediately on save / config
  change). `run_cycle` already batches multiple changed paths per cycle.
- The **workspace root** (first workspace folder) is the server's `cwd` for
  config discovery — same semantics as running the CLI in the project root.
- **Untitled documents** (no file path) and files outside the project are
  ignored: no overlay, no diagnostics.
- The server never touches `notify`: the LSP client watches the config files
  (E8) and open buffers arrive as protocol events. Files changed *outside* the
  editor (e.g. `git pull`) are picked up on the next cycle only if a watched
  event fires — v1 accepts this (documented); a low-frequency full-recheck
  timer can be added later if it bites.

## 3. Engine changes: buffer overlays (in `crates/cli`)

Core `extract()` needs no changes (it already takes `&str`). The work:

- **`WatchSession` gains overlay methods** — `set_overlay(path, content)`,
  `clear_overlay(path)` — since the runner internals are `pub(crate)` and must
  stay behind its API. Overlay entries carry a **monotonic version counter**.
- **`runner.rs`**: `extract_one()`/`extract_with_cache()` consult the overlay map
  before `fs::read_to_string`; for overlaid files the mtime/size `ExtractionCache`
  validity key is replaced by the overlay version (stat-based caching assumes
  "disk is truth" — the one place needing real design care, see risk R1).
- **`report.rs`**: `read_cached()` (line/col rendering) must read the same
  overlay content, or diagnostic positions would be computed against stale disk
  text while spans came from the buffer.
- `clear_overlay` re-dirties the file so the next cycle re-extracts from disk.

Non-goal: overlays in the CLI watch mode or one-shot lint — the API exists on
`WatchSession`, but only the LSP populates it.

## 4. VS Code extension (`editors/vscode/`)

- **Stack**: TypeScript, `vscode-languageclient` (v9+), esbuild single-file
  bundle, `@vscode/vsce` for packaging. Extension name `ImportLint`; publisher
  account is a one-time runbook step (reserve `uhyo` or `import-lint`).
- **Activation**: `onLanguage:{typescript,typescriptreact,javascript,javascriptreact}`
  + `workspaceContains:**/.importlintrc.{json,jsonc}`; the E9 `enabled` gate then
  decides whether to actually spawn the server.
- **Settings**: `importLint.binaryPath` (E6), `importLint.run` (E4),
  `importLint.enabled` (E9), `importLint.trace.server` (languageclient standard).
- **Commands**: `importLint.restart` (stop client, re-locate binary, restart —
  also the recovery path after `npm install` swaps the binary version).
- **Locator** is a small pure module (input: workspace root, platform, settings;
  output: binary path or a structured "not found" reason) so it's unit-testable
  with `node --test`, mirroring the npm shim's testing approach. Reuse the shim's
  platform-key logic (glibc/musl probe) rather than reimplementing it.
- WSL2 note (dev environment): under VS Code Remote-WSL the extension runs in
  the WSL extension host, so `node_modules` resolution and binary spawn are
  native-Linux — no special handling.

## 5. Testing strategy

- **Overlay engine tests** (`crates/cli/tests/watch.rs` + unit tests): overlay
  set/edit/clear cycles, cache-invalidation edges (overlay hides disk edit;
  clear falls back to disk; version bump forces re-extract), cross-file case
  (open unsaved `b.ts` re-export change flips a diagnostic in `a.ts`).
- **LSP protocol tests** (`crates/cli/tests/lsp.rs`): drive the real server loop
  over `lsp_server::Connection::memory()` — initialize handshake, didOpen →
  publishDiagnostics, didChange with a violation-introducing edit → diagnostics
  appear for a *different* file, didClose → reverts to disk state, config-change
  → republish, clearing semantics (empty array). No editor needed; runs in CI on
  all three OSes via the normal test job.
- **Extension**: locator unit tests (`node --test`); a smoke test with
  `@vscode/test-electron` is stretch goal, not a gate — the protocol tests
  above cover the server, and the extension is deliberately thin.
- **Manual gate before first publish**: install the vsix locally, verify
  end-to-end on the dev machine (WSL2) against a real project.

## 6. Risks and mitigations

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | Overlay vs `ExtractionCache` invalidation bug (stale cache hit ⇒ edits silently ignored; over-invalidation ⇒ keystroke thrash) | Wrong diagnostics or lag | Version-counter design in §3, dedicated cache-edge tests in §5; the fast path already re-extracts only changed files, bounding thrash. |
| R2 | `lsp-types` staleness blocks a needed protocol feature | Feature stall | We use only LSP 3.16-era features (publishDiagnostics, watched files, dynamic registration). If it ever binds, switch to `tower-lsp-server`'s vendored types — server logic is behind our own module boundary. |
| R3 | Publish-all floods the Problems panel in huge repos | UX noise | It mirrors what the CLI prints today, so it's consistent; if reports demand it, add `importLint.reportClosedFiles: false` later — the diff/publish layer makes that a filter, not a redesign. |
| R4 | Marketplace auth churn (global Azure DevOps PATs retire Dec 2026) | CI publish breaks | Use a publisher-scoped Marketplace PAT now; note the Entra-ID migration in RELEASING.md so it's a known follow-up, not a surprise. |
| R5 | Binary/extension version skew (old CLI in node_modules lacks `lsp` subcommand) | Confusing startup failure | Extension runs `import-lint --version` at locate time; if the binary predates the LSP (< the L2 release version), show one clear "upgrade @import-lint/cli" notification instead of spawning. |
| R6 | Multi-root workspaces silently mislint | Wrong project root | E10: first root wins + explicit warning notification when more roots exist; documented limitation. |

## 7. Milestones

**L1 — Buffer overlays in the engine:** §3 complete (`WatchSession`
overlay API, runner/report plumbing, version-keyed cache bypass), overlay tests
green, zero behavior change when no overlays are set (existing 158+ tests
untouched). Exit: cross-file overlay test passes — an unsaved edit in one buffer
moves a diagnostic in another file.

**L2 — `import-lint lsp` server:** §2 complete behind a new clap subcommand;
protocol integration tests per §5 green on Linux/macOS/Windows CI. Exit: a
generic LSP client (protocol tests + a manual neovim or VS Code smoke) gets
live cross-file diagnostics with overlays, config hot-reload, and clean
publish/clear behavior.

**L3 — VS Code extension:** `editors/vscode/` per §4 — locator + client wiring
+ settings + restart command, packaged vsix installs and works end-to-end
locally (WSL2 manual gate). Exit: fresh project + `npm install -D
@import-lint/cli` + local vsix ⇒ squiggles on violations as you type.

**L4 — Publish & docs:** extension CI (lint/build/test in `ci.yml`),
`vscode-release.yml` on `vscode-v*` tags (vsce + ovsx, `--skip-duplicate`),
one-time runbook steps in RELEASING.md (Marketplace publisher + scoped PAT,
Open VSX namespace + Eclipse agreement), root README "Editor integration"
section + npm README pointer. Exit: extension live on Marketplace and Open VSX,
installable by name, docs updated.

## 8. Explicitly out of scope

- Code actions / quick fixes (e.g. "add `@public`"), hover, go-to-definition —
  future milestone once diagnostics are stable.
- Pull diagnostics, UTF-8/UTF-32 position-encoding negotiation (E5).
- Multi-root workspace support beyond first-root-wins (E10).
- Editor plugins beyond VS Code — the LSP server is editor-agnostic and a
  README snippet for neovim/helix config is docs work, not a plugin.
- Native Node bindings (napi-rs) — the process boundary + LSP is the
  integration surface.
- JUnit output and opt-in strict checks (`export *`/dynamic import/require) —
  separate post-v1 ideas, not part of M8.
