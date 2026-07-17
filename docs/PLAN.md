# ImportLint — Newcomer Documentation Plan (M10)

All shipped so far: core linter (M0–M7, [`PLAN-v1.md`](./PLAN-v1.md)), npm
distribution (N1–N3, [`PLAN-npm.md`](./PLAN-npm.md)), LSP + VS Code extension
(M8, [`PLAN-lsp.md`](./PLAN-lsp.md)), and the `init` scaffolding subcommand
(M9, [`PLAN-init.md`](./PLAN-init.md)).

**Problem (from the 2026-07-17 docs audit):** the README is reference
documentation for people who already use `eslint-plugin-import-access`. It
opens by *comparing* ("same functionality as eslint-plugin-import-access,
without tsc/ESLint") rather than *teaching*. The `@package` concept, the word
"package" itself, "importability", the loopholes, and one-hop re-export
semantics are all used before (or without ever) being defined. There is no
motivation section, no concrete violation example, no tutorial, and the three
`init` presets are named but not choosable by someone who doesn't already know
the convention behind each.

**Goal:** a developer who has never heard of `eslint-plugin-import-access` (or
JSDoc access tags at all) can, from the README alone, understand *why* the tool
exists, see a violation and its fix in 30 seconds of reading, get running with
`import-lint init`, and find their way to deeper guides. Migrator content
stays, but stops being the organizing principle.

**Scope (user-locked):** newcomer-first README overhaul + a small in-repo
guide set under `docs/guides/`. Explicitly **no docs-site infrastructure**
(no VitePress/mkdocs/GitHub Pages).

---

## 1. Decisions (locked)

| # | Decision | Rationale |
|---|---|---|
| D-D1 | **README leads with teaching, not comparison.** New section order: pitch (one paragraph, self-contained) → "Why?" (the file-is-the-only-encapsulation-unit problem, adapted from the reference plugin's framing) → a concrete example (real files, a `@package` export, the violating import, the *actual* error output) → Getting started (`init`) → reference sections (flags/config/output/CI/watch/editors) → Migration → Performance/Roadmap. A "already migrating from eslint-plugin-import-access?" jump-link sits right under the pitch so migrators lose nothing. | The audit's core finding: every reader currently pays the migrator tax. Migrators are served by one link; newcomers can't be served by any link if the concept is never taught. |
| D-D2 | **Three guides in `docs/guides/`**: `concepts.md` (the mental model: packages, importability, boundaries, index/filename loopholes, `packageDirectory`, one-hop re-export semantics — the glossary lives here), `tutorial.md` (hands-on: scaffold with the `standard` preset, create a `foo.package/` boundary, hit a real error, fix it three ways — `@public`, index re-export, move the importer inside), `adoption.md` (preset comparison table + playbooks: greenfield/standard, retrofit/gradual with a phased-annotation strategy, monorepo). | One doc per question a newcomer actually asks: "how does this think?", "show me", "which preset and how do I roll it out?". Three files, flat, no infra (scope lock). `docs/guides/` keeps user docs visibly separate from internal `PLAN*/RELEASING/research`. |
| D-D3 | **Every command, code sample, and error message shown in any doc must be produced by actually running the workspace binary** and pasted from real output (paths/timing scrubbed). No hypothetical output. | The docs teach by example; a fabricated error string that drifts from `diagnostics.rs` poisons trust. This is the doc-equivalent of M9's template round-trip test — enforced editorially (§3 checklist) rather than in CI. |
| D-D4 | **Diagrams are ASCII file-trees and annotated code blocks only** — no images, no mermaid. | The domain is directories and imports; a file-tree with arrows in a code fence renders identically on GitHub, crates.io, npmjs.com, and in terminals. The reference plugin's PNGs are its weakest maintenance point. |
| D-D5 | **README stays a complete standalone reference** (target ≤ ~450 lines): guides deepen, they don't replace. Nothing that exists only in a guide may be *required* to use the tool correctly; config-option one-liners stay in the README and link to `concepts.md` for the "why". | Most readers never leave the README; npm/crates render it as the package page. Guides are for the second sitting. |
| D-D6 | `npm/import-lint/README.md` and `editors/vscode/README.md` open with the same one-paragraph *teaching* pitch as the root README (not the "port of eslint-plugin-import-access" comparison), then link to the root README. GitHub repo description + npm `description` field get the same treatment (repo description is a manual user step; npm description changes in `npm/import-lint/package.json` ride the next publish). | The audit found the comparison-as-identity problem replicated in every entry point. One pitch, written once, reused verbatim. |
| D-D7 | **Migration section is kept intact** (content unchanged apart from moving later and gaining the jump-link target). | It's good content for its audience; the problem was placement, not existence. |

## 2. Content requirements (what "done" means per doc)

- **README pitch:** must be understandable with zero prior context — defines
  the idea (mark an export `@package` to keep it inside its directory-package)
  in the first two sentences; mentions speed and the no-tsc/no-ESLint design as
  *properties*, not as the identity.
- **README "Why?":** the encapsulation-stops-at-the-file problem, ~3 short
  paragraphs max, ending with "ImportLint adds a directory-level layer".
- **README example:** one file-tree + two code snippets + the real `pretty`
  diagnostic, then the one-line fix. Must fit on one screen.
- **concepts.md:** defines, in order, with an example each: *package*,
  *importability* (`@public`/`@package`/`@private` + `defaultImportability`),
  *package directory* (default parent-dir behavior vs `packageDirectory`
  globs, incl. the `*.package` convention), *index loophole* (incl. the
  promotion-cascades-one-level-at-a-time behavior), *filename loophole*,
  *re-exports and one-hop semantics* (a bare re-export resets to
  `defaultImportability`; the re-export's own JSDoc governs), what counts as
  *external* (never checked) vs *internal*. Semantics must match the shipped
  rule engine — derive examples from `crates/cli/tests/` fixtures and verify
  per D-D3.
- **tutorial.md:** every step copy-pasteable in an empty directory; total time
  ~10 minutes; ends with the reader having seen error → three distinct fixes →
  green run, plus a "where next" footer (concepts, adoption).
- **adoption.md:** a comparison table a newcomer can choose from (what's a
  boundary, what's restricted by default, best for), then one playbook per
  preset; the `gradual` playbook must give an actual phasing strategy
  (annotate one directory, `--format` in CI, ratchet).

## 3. Verification checklist (release-gate for each milestone)

1. Every shell block ran verbatim against the current workspace binary; every
   diagnostic shown is pasted real output.
2. Every config snippet parses (`import-lint --config <snippet>` on a temp
   copy, or via an `init`-generated base).
3. Every claimed behavior (loophole promotion, one-hop reset, external
   exemption) is demonstrated by a fixture that was actually run.
4. Terms are defined at first use in each doc (docs are entered independently
   via search).
5. All cross-links resolve on GitHub (relative paths from each file's own
   location; remember npm/crates render the README *outside* the repo — links
   from the root README to `docs/guides/*` must be absolute GitHub URLs, same
   as the existing Migration-section convention if one exists, else
   `https://github.com/uhyo/import-lint/blob/master/...`).
6. `cargo test --workspace` still green (docs-only, but the RELEASING.md R-I4
   prose-sync rule now extends to: README config example ↔ `gradual` template
   ↔ any config shown in guides).

## 4. Risks

| # | Risk | Mitigation |
|---|---|---|
| R-D1 | Subtle-semantics errors in `concepts.md` (one-hop, loophole cascade) | D-D3 run-everything rule; examples derived from conformance fixtures; final review by the project lead against the rule-engine source. |
| R-D2 | README bloat (teaching added, nothing removed) | D-D5 line budget; the deep option-reasoning moves to `concepts.md`, terse one-liners remain. |
| R-D3 | Relative links broken on npm/crates package pages | §3.5 absolute-URL rule for README→guides links. |
| R-D4 | Guides drift as options evolve | Guides carry a "documents behavior as of vX.Y.Z" line; RELEASING.md checklist item extended (§3.6). |

## 5. Milestones

**D1 — guides:** `docs/guides/{concepts,tutorial,adoption}.md` per §2, all
examples verified per §3. Exit: a reviewer can execute `tutorial.md` top to
bottom in an empty temp dir and every output matches.

**D2 — README overhaul + peripheral pitches:** root README restructured per
D-D1/D-D5 with links into the guides; npm + vscode READMEs and npm package
`description` per D-D6; RELEASING.md checklist extension (§3.6). Exit: the
audit's §C gap list re-checked — each of the 8 gaps has a concrete answer;
README ≤ ~450 lines; migration content intact behind the jump-link.

## 6. Out of scope

- Docs website / GitHub Pages (user-locked).
- Screenshots, GIFs, images of any kind (D-D4).
- Translating docs (the reference plugin has Japanese docs; nothing here yet).
- A `docs/` restructure of internal files (`PLAN*`, `RELEASING`, `benchmarks`
  stay where they are).
- CI enforcement of doc examples (editorial checklist only, v2 idea: a
  doc-snippet extraction test).
