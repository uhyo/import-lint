# ImportLint — `init` Scaffolding Plan (M9)

> **Status update (2026-07-18, after v0.1.3):** presets were removed. `init`
> now always writes the one template that was the `standard` preset, with no
> `--preset` flag and no interactive picker — the `packageDirectory`
> project-root fallback makes that config suit gradual adoption too, so the
> `gradual` and `monorepo` presets became config-edit recipes in
> `docs/guides/adoption.md` instead. D-I2 (scaffold-time-only), D-I3/D-I4
> (commented `.jsonc` from a static, round-trip-tested string), D-I6
> (overwrite safety), and D-I7 (stderr-only) still stand; the preset-selection
> parts of D-I2 and all of D-I5 are superseded. The rest of this document is
> kept as designed for the historical record.

Everything planned so far has shipped: the core linter (M0–M7), npm distribution
(N1–N3), and the LSP server + VS Code extension (M8/L1–L4). Earlier plans are
archived at [`docs/PLAN-v1.md`](./PLAN-v1.md), [`docs/PLAN-npm.md`](./PLAN-npm.md),
and [`docs/PLAN-lsp.md`](./PLAN-lsp.md).

Today, adopting ImportLint means hand-writing `.importlintrc.jsonc` — in practice,
copying the README's example and guessing which options fit your project. This plan
adds a scaffolding subcommand:

**Goal:** `import-lint init` creates a ready-to-run, fully commented
`.importlintrc.jsonc` in the current directory, offering a small set of **presets**
chosen either interactively (TTY) or non-interactively via a `--preset` flag, so
that `npx @import-lint/cli init && npx @import-lint/cli` is a working first-run
experience.

---

## 1. Product decisions (locked)

| # | Decision | Rationale |
|---|---|---|
| D-I1 | `init` is a subcommand of the existing binary — **`import-lint init [--preset <name>] [--force]`** — and writes `.importlintrc.jsonc` into the **current directory** (which thereby becomes the project root). | Same single-binary distribution story as `lsp` (PLAN-lsp.md E1): no new crate, no new packages, works identically via crates.io, npm shim, and GH-Release binaries. Writing to cwd matches config-discovery semantics exactly (the config file's directory *is* the project root, PLAN-v1.md D7) — no `--dir` flag to reason about. |
| D-I2 | **Three presets: `standard`, `gradual`, `monorepo`** (see §2). A preset is a *scaffold-time template*, not a runtime mode: the generated file is plain config, fully editable, and contains no reference back to the preset (no `"extends"`). | The `jsdoc` rule has two axes that actually distinguish real-world setups (`defaultImportability`, `packageDirectory`); three presets cover them without sprawl. Scaffold-time-only keeps `crates/core` completely untouched — the config model, loader, and LSP know nothing about presets. *(Revised in review: default-package is the flagship for new projects, but bare default-package with every directory as a boundary is too strict to recommend — `standard` adopts the `*.package` naming convention recommended in the reference plugin's v3.1.0 `packageDirectory` writeup, see §2.)* |
| D-I3 | Always write **`.importlintrc.jsonc`**, never `.json`, and every preset's output is **fully commented** — each option annotated the way the README example is. | `.jsonc` wins discovery over `.json` (D7), and comments are the product: the generated file doubles as inline documentation, which is what makes a template better than `{}`. |
| D-I4 | Templates are **static string constants** in `crates/cli/src/init.rs`, one per preset — no serde serialization, no templating engine. Guard: a unit test parses every template through `LintConfig::load`. | Comments can't come out of `serde_json`, so string constants are forced anyway; the win is that `deny_unknown_fields` turns any future config-schema drift into a red test instead of a scaffold that fails at first lint. |
| D-I5 | Interactive selection runs when `--preset` is absent **and stdin and stderr are both TTYs**: a hand-rolled **numbered menu** (prompt on stderr, one line read from stdin; empty input = `standard`; invalid input re-prompts; EOF is an error). Non-TTY without `--preset` exits `2` with a "pass `--preset <name>`" message. | No new dependencies: arrow-key pickers (`dialoguer`, `inquire`) need raw terminal mode, which is untestable in CI and overkill for three options. A numbered menu is a pure function over a reader/writer, unit-testable with a `Cursor`. Erroring (rather than silently defaulting) in non-TTY contexts matches the config loader's explicit-over-implicit philosophy; CI/scripts have `--preset`. |
| D-I6 | Overwrite safety: if `.importlintrc.jsonc` **or** `.importlintrc.json` already exists in cwd, refuse with exit `2` unless `--force`. `--force` writes `.jsonc` and, when a `.json` remains beside it, prints a note that the `.jsonc` now shadows it. If a config exists only in an *ancestor* directory, proceed, but print a note that the new file takes over for this subtree. | Never destroy a config silently. The ancestor case is legitimate (initializing a nested project) but surprising enough to call out, since the new file changes the project root for everything under cwd. |
| D-I7 | All `init` output (prompt, notes, success message) goes to **stderr**; stdout stays clean. Exit codes follow the CLI contract: `0` file created, `2` everything else (existing config without `--force`, non-TTY without `--preset`, I/O failure). | The lint invocation reserves stdout for diagnostics; `init` printing human chatter to stderr keeps the binary's stream contract uniform and `init`'s output pipe-safe. |
| D-I8 | No filesystem sniffing in v1: `init` does not detect `tsconfig.json`, package managers, or workspace layouts. The template carries the commented-out `"tsconfig"` line; the existing `<project root>/tsconfig.json` default already covers the common case at lint time. | Keep `init` dumb and predictable. Detection (e.g. pre-filling `packageDirectory` from package.json `workspaces`) is a plausible v2, listed in §7 — it should be added deliberately, not smuggled into v1. |

## 2. Presets

| Preset | Distinguishing config | Who it's for |
|---|---|---|
| `standard` | `"defaultImportability": "package"` + `"packageDirectory": ["**/*.package"]` | **New projects (recommended, the picker default): the `*.package` naming convention** — a *meta-configuration* (a naming rule, not a directory list, so the config never needs updating as the project grows). A directory named `foo.package/` is a hard encapsulation boundary: everything inside it, at any depth, imports freely from anything else inside; nothing outside can import from it unless the export is tagged `@public` (or surfaced through the boundary's `index.ts` — the index loophole, on by default, promotes `index.ts` exports to the parent's package, and since a bare re-export resets to `defaultImportability`, exposure cascades deliberately one level at a time). Boundaries are visible in the file tree itself. *Outside* any `*.package` directory, the package-directory fallback is the file's own parent, so default-package still means per-directory scoping there, with the same `index.ts` cascade as the escape valve — declare a `*.package` boundary above when a subtree should share freely. |
| `gradual` | All defaults (`defaultImportability: "public"`, `indexLoophole: true`) | Incremental adoption on an existing codebase: nothing is restricted until you tag exports `@package`/`@private`. The generated file is essentially the README example. |
| `monorepo` | `"defaultImportability": "package"` + `"packageDirectory": ["packages/*"]` (with a comment saying to adjust the globs, e.g. adding `"apps/*"`) | Workspace repos: boundaries sit at the workspace-package level — deep relative reach-ins across sibling packages (`../../other-pkg/src/…`) become errors unless the export is `@public`, while name-based imports of sibling workspace packages resolve through `node_modules` and stay exempt as external (and `treatSelfReferenceAs` keeps its `"external"` default). Inside one workspace package, imports are unrestricted. |

Each template is a complete, self-describing config: `include`, `exclude`, the
commented `tsconfig` line, and the full `rules.jsdoc` block with every option
present (commented out when it's at its default), adapted from the README's config
example. The preset determines which lines are live and what values they hold —
nothing else differs. The `standard` template's comments also teach the workflow
(name a boundary `foo.package/`; publish via `index.ts` re-export or `@public`)
and show the documented alternative conventions as commented-out one-liners so
switching is a single edit: the inverse naming rule (`["**", "!**/*.internal"]` —
every directory is a boundary *except* ones opting out by name), the fixed-location
style (`["src/packages/*"]`), and `"filenameLoophole": true` for the companion-file
pattern (`sub.ts` next to `sub/`).

## 3. CLI surface

```
import-lint init                    # interactive picker (TTY); exit 2 if not a TTY
import-lint init --preset gradual   # non-interactive, for scripts/CI
import-lint init --force            # overwrite an existing config in cwd
```

- `--preset` is a clap `ValueEnum` — invalid names get clap's native error and the
  candidates list for free, and the presets self-document in `init --help`.
- Interactive transcript sketch (stderr):

  ```
  Choose a preset:
    1) standard  — *.package naming convention: directories named foo.package
                   are encapsulation boundaries; exports are package-scoped
                   unless @public (recommended for new projects)
    2) gradual   — annotation-driven: exports stay public until tagged
                   @package/@private (for adopting on an existing codebase)
    3) monorepo  — boundaries at packages/*: no relative reach-ins across
                   workspace packages
  Preset [1]:
  ```

- Known (accepted) behavior change: a lint target literally named `init` now needs
  `import-lint ./init`, same as the existing `inspect`/`graph`/`lsp` names.

## 4. Implementation shape (`crates/cli/src/init.rs`)

- `Preset` enum (`clap::ValueEnum` + a description used by both `--help` and the
  menu), `fn template(Preset) -> &'static str` over three `const` strings.
- `run_init(cwd, preset: Option<Preset>, force: bool) -> Result<(), InitError>` —
  the guards from D-I6, the write, the notes. `main.rs` gains a
  `Command::Init { preset, force }` arm that maps `InitError` to exit `2`,
  mirroring the other subcommands.
- `choose_preset(input: impl BufRead, out: impl Write) -> io::Result<Preset>` —
  the menu as a pure function; the TTY gate lives in the caller only. This seam is
  what makes the interactive path unit-testable (and swappable for a fancier
  picker later, see risk R-I3).
- Zero changes to `crates/core`.

## 5. Testing strategy

- **Template round-trip (unit):** every preset's template must parse via
  `LintConfig::load` — with `deny_unknown_fields`, any config-schema change that
  invalidates a template is a red test, not a broken scaffold (D-I4). Also assert
  the distinguishing options landed (e.g. `standard` really has
  `defaultImportability: Package` and `packageDirectory: ["**/*.package"]`).
- **Menu (unit):** `choose_preset` over `Cursor` inputs — picks by number, empty
  line defaults to `standard`, garbage re-prompts then succeeds, EOF errors.
- **CLI integration (`crates/cli/tests/init.rs`):** for each preset,
  `import-lint init --preset X` in a temp dir creates the file **and a follow-up
  lint run in that fixture succeeds using it** (the real exit criterion); refusal
  + exit `2` when `.importlintrc.jsonc` or `.json` exists; `--force` overwrites;
  non-TTY without `--preset` exits `2` with the guidance message. The interactive
  path is deliberately not e2e-tested (piped stdin never passes the TTY gate) —
  that's what the `choose_preset` seam is for.
- **Docs:** README Quick start gains `import-lint init` as step 1; the config
  section mentions the presets; npm README gets the same one-liner.

## 6. Risks and mitigations

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R-I1 | Template drift as the config schema evolves | `init` scaffolds a config that fails to load | Round-trip unit test per preset (D-I4); `deny_unknown_fields` makes drift loud. |
| R-I2 | Preset sprawl / naming bikeshed | Maintenance burden, decision fatigue for users | Presets are curated starting points, not a plugin surface: adding one costs a const + a test row, and there is deliberately no runtime `"extends"` to keep compatible forever. |
| R-I3 | Numbered menu feels dated next to arrow-key CLIs | UX polish complaints | Fine for three options; if demand materializes, swap the implementation behind `choose_preset` for `dialoguer` — one function, no architectural change. |
| R-I4 | README example and the `gradual` template drift apart | Docs inconsistency (not breakage) | Editorial: they're adapted from the same text; the round-trip test guards correctness, and a release-checklist line in RELEASING.md covers the prose. |

## 7. Milestones

**I1 — `init --preset` end-to-end:** subcommand + presets + templates + the D-I6
guards + unit/integration tests per §5 + README updates. Exit: in a fresh temp
project, `import-lint init --preset monorepo && import-lint` runs green using the
generated config on all three CI OSes.

**I2 — interactive picker:** TTY gate + numbered menu + `choose_preset` unit
tests + docs polish. Exit: manual TTY smoke — bare `import-lint init` in a fresh
project walks the prompt and scaffolds the `standard` preset.

## 8. Explicitly out of scope

- Runtime preset semantics (`"extends": "standard"` resolved at lint time) —
  presets exist only at scaffold time (D-I2).
- Project detection: reading package.json `workspaces` to pre-fill
  `packageDirectory`, tsconfig discovery, framework sniffing (D-I8) — the natural
  v2 if `init` gets traction.
- Prompting beyond the single preset question (include dirs, severity, rule
  options) — the generated file's comments do that job better than a wizard.
- A VS Code "ImportLint: Initialize" command wrapping `init` — editor work,
  separate decision.
- Writing `.importlintrc.json` (comment-free) output — `.jsonc` only (D-I3).
