# Conformance suite

This directory is the oracle for ImportLint's core semantics (docs/PLAN-v1.md §9.1).
It removes ambiguity about "does the reference plugin flag this?" by generating
the reference's own answer, rather than by re-deriving it from the spec doc.

- `fixtures/` — a copy of `eslint-plugin-import-access`'s own test fixture
  project (`src/__tests__/fixtures/project/` and `src/__tests__/fixtures/packages/`),
  pinned at v3.1.0.
- `expected/` — one JSON file per exercised option set, containing every
  diagnostic the reference plugin produces when linting **every** `*.ts` file
  under `fixtures/project/src/` with that option set, plus `manifest.json`
  mapping each snapshot file to its exact options object.
- `oracle/generate-snapshots.mjs` — the Node script that produces `expected/`
  by running the reference plugin (built, `dist/`) directly against a
  reference-repo checkout. It is **not** shipped or run as part of the Rust
  build; it's a one-off generator, re-run only when the reference plugin's
  behavior needs to be re-captured.

Rust integration tests lint `fixtures/project/src/` with ImportLint under the
same option sets and diff the resulting diagnostics against `expected/*.json`.

## Diagnostic JSON shape

```jsonc
{
  "file": "src/class/barUser.ts",   // relative to fixtures/project/, forward slashes
  "line": 1, "column": 10, "endLine": 1, "endColumn": 26,
  "messageId": "package",           // "package" | "package:reexport" | "private" | "private:reexport"
  "message": "Cannot import a package-private export 'barAccessPackage'",
  "identifier": "barAccessPackage"  // extracted from the trailing '...' in message
}
```

Sorted by `(file, line, column, messageId)`.

## Regenerating the snapshots

Requires a checkout of `eslint-plugin-import-access` with `npm install` already
run (so `node_modules` — including the `file:`/workspace fixture packages
under `devDependencies` — is populated; see "Fixture package installation"
below).

```sh
node tests/conformance/oracle/generate-snapshots.mjs /path/to/eslint-plugin-import-access
# or:
REFERENCE_REPO=/path/to/eslint-plugin-import-access node tests/conformance/oracle/generate-snapshots.mjs
```

**After regenerating**: the script writes the oracle's verbatim output, which
for `package-directory-packages-glob` is NOT what ImportLint should produce —
re-apply the documented divergence below to that one file (or restore it from
git and diff by hand) before committing.

Defaults to `/home/uhyo/repos/eslint-plugin-import-access` if no path is given.
The script:

1. Verifies the checkout's `package.json` name/version is
   `eslint-plugin-import-access@3.1.0` (hard-fails otherwise — bump the
   pinned version in the script deliberately if the reference plugin is
   intentionally upgraded, then re-verify every snapshot).
2. Runs `npm run build` in the reference repo to produce a fresh `dist/`
   (`dist/` is gitignored there; skip with `SKIP_BUILD=1` if you know it's
   current).
3. Enumerates fixture files via `git -C <reference-repo> ls-files
   src/__tests__/fixtures/project/src` (never a filesystem walk — avoids
   picking up untracked scratch files another process might be writing into
   that checkout).
4. For each option set (see below), lints every enumerated file with
   `TSESLint.Linter` + `@typescript-eslint/parser` (`projectService: true`,
   mirroring `src/__tests__/fixtures/eslint.ts`'s `FlatESLintTester` exactly)
   and the reference's **built** `dist/rules/jsdoc.js` rule.
5. Writes `expected/<name>.json` + `expected/manifest.json`.

The script never writes inside the reference repo checkout (it only reads
files and runs `npm run build`, whose output is gitignored) and asserts the
reference repo's `git status --porcelain` didn't pick up unexpected changes.

## Option sets captured

Every distinct options object passed to `tester.lintFile(...)` across
`src/__tests__/*.ts` in the reference repo (excluding the fixtures
themselves), enumerated by grepping every `lintFile(` call site:

| snapshot | options | diagnostics |
|---|---|---|
| `default` | `{}` | 30 |
| `index-loophole-false` | `{ indexLoophole: false }` | 33 |
| `index-loophole-false-filename-loophole-true` | `{ indexLoophole: false, filenameLoophole: true }` | 30 |
| `default-importability-package` | `{ defaultImportability: "package" }` | 34 |
| `default-importability-package-exclude-source-patterns` | `{ defaultImportability: "package", excludeSourcePatterns: ["src/exclude-patterns/types/**"] }` | 33 |
| `default-importability-private` | `{ defaultImportability: "private" }` | 35 |
| `default-importability-private-self-reference-internal` | `{ defaultImportability: "private", treatSelfReferenceAs: "internal" }` | 36 |
| `default-importability-private-self-reference-external` | `{ defaultImportability: "private", treatSelfReferenceAs: "external" }` | 35 |
| `package-directory-no-internal` | `{ packageDirectory: ["**", "!**/_internal"] }` | 28 |
| `package-directory-all-star` | `{ packageDirectory: ["**"] }` | 30 |
| `package-directory-no-internal-filename-loophole` | `{ packageDirectory: ["**", "!**/_internal"], filenameLoophole: true }` | 25 |
| `package-directory-packages-glob` | `{ packageDirectory: ["src/package-directory/packages/*"] }` | 3 (†) |

(†) `package-directory-packages-glob` is the one snapshot that is **not** the
oracle's verbatim output — see "Documented divergences" below.

## Documented divergences from the reference plugin

ImportLint deliberately diverges from the reference in one case: when
`packageDirectory` is set and a file has **no** matching ancestor directory, the
reference falls back to the file's own parent directory (resurrecting
directory-per-package semantics for every unmatched file), while ImportLint
falls back to the **project root**, so all files outside every configured
boundary share one project-wide package. This makes gradual adoption of a
naming convention like `["**/*.package"]` possible (see
`crates/core/src/rule/in_package.rs` module docs).

Consequence for `package-directory-packages-glob` (the only captured option set
whose patterns leave some fixture files unmatched — the `**`-based sets always
match an ancestor): the oracle produces 29 diagnostics, ImportLint 3. The
26 dropped diagnostics are exactly the `package`/`package:reexport` violations
where importer and exporter both live outside
`src/package-directory/packages/*` — same root package now, so allowed. The
3 kept are the two `private` diagnostics (never affected by package
boundaries) and `packages/packageB/crossUser.ts` reaching into `packageA`
(both inside matched boundaries). The checked-in
`expected/package-directory-packages-glob.json` records ImportLint's intended
output, hand-derived from the oracle's by applying that rule. (This also makes
the fixture comment in `src/package-directory/crossPackageUser.ts` — "should
fail even with packageDirectory option" — stale for *this* option set; the
fixture tree is a pinned copy of the reference's and is left unmodified.)

Note: `default-importability-private-self-reference-external` is byte-for-byte
identical to `default-importability-private` — `treatSelfReferenceAs:
"external"` is the default, so passing it explicitly changes nothing. It's
kept as its own snapshot anyway (rather than deduplicated away) because the
reference test suite exercises it as an explicit case
(`src/__tests__/self-reference.ts`), and a Rust implementation bug that
mishandles an *explicitly-set* default differently from an *implicit* default
would otherwise go undetected.

No option set from the reference tests was skipped.

## Fixture package installation (third-party / workspace symlinks)

`fixtures/project/src/library/*` and `fixtures/project/src/self-reference/*`
import packages that don't exist as ordinary `node_modules` entries checked
into git — they come from the reference repo's own root `package.json`:

- `devDependencies` with `"file:src/__tests__/fixtures/packages/third-party/*"`
  — installed by npm as **real copies** into
  `node_modules/@fixture-package-third-party/*` (confirmed: these are plain
  directories, not symlinks).
- root `"workspaces": ["src/__tests__/fixtures/packages/workspaces/*"]` — npm
  workspaces always **symlinks** these into
  `node_modules/@fixture-package-workspace/*` (confirmed via `readlink`).

This symlink-vs-copy distinction is exactly what D5/spec §4/library.ts's
"Workspace modules (symlink)" tests are pinning down: external-vs-internal
classification must be resolution-provenance-based, not a `node_modules`
path-substring check, because TypeScript resolves the workspace symlinks to
their real path (outside any `node_modules` segment) before classifying them.

**This whole node_modules layout lives outside the fixture tree copied into
`fixtures/`** (it's a root-level install artifact of the reference repo, not
tracked by git, and not scoped under `src/__tests__/fixtures/`). The oracle
script only reproduces it correctly because it lints the reference repo's
**real, in-place, `npm install`-ed** checkout — never a copy.

Consequence for the Rust side: `fixtures/packages/{third-party,workspaces}/*`
in this directory are the **source** package directories only. Resolver
integration tests that need the third-party/workspace-symlink resolution
behavior (docs/PLAN-v1.md §9.3, spike S3) must construct their own
`node_modules/@fixture-package-{third-party,workspace}/*` layout next to
`fixtures/project/` — real copies for `third-party/*`, symlinks for
`workspaces/*` — rather than assuming one is checked in here. None of the 12
captured option-set snapshots above depend on this being present in this
repo's checkout, since the diagnostics were captured directly from the
reference repo where the layout already exists.

## Symlinks in the fixture tree itself

None. Verified via `git ls-files -s` (mode `120000` = symlink) over
`src/__tests__/fixtures/project` and `src/__tests__/fixtures/packages` in the
reference repo — zero matches. All symlinks involved in this suite are the
node_modules workspace links described above, which are install-time
artifacts, not fixture-tree content.

## Spot-checks

Cross-checked against the reference repo's own hardcoded `toMatchInlineSnapshot`
/ `toEqual` expectations in its test files (all passed exactly):

- `classes.ts` → `src/class/barUser.ts`: 3 diagnostics (`barAccessPackage`
  col 10–26, `barPackage` col 28–38, `barPackage` col 40–61, all `package`) —
  matches `default.json`.
- `reexport.ts` → `src/reexport/useFoo.ts`: 1 diagnostic (`subFooPrivate`
  col 18–31, `private`) — matches `default.json`.
- `reexport.ts` → `src/reexport/useBaz.ts`: 1 diagnostic (`subBaz` col 10–16,
  `private`) — matches `default.json`.
- `exclude-patterns.ts`: `generated-type-user.ts` has the `someValue`/`package`
  diagnostic (col 10–19) under `default-importability-package.json`, and none
  under `default-importability-package-exclude-source-patterns.json` —
  matches both `toMatchInlineSnapshot` and `toEqual([])` respectively.
- `package-directory.ts`: no diagnostics for `_internal`-subdirectory imports
  under `package-directory-no-internal.json` — matches `toEqual([])`.
- `self-reference.ts`: `src/self-reference/user.ts` has the `exportedValue`/
  `private` diagnostic (col 10–23) under
  `default-importability-private-self-reference-internal.json`, and none
  under `default-importability-private.json` — matches both inline snapshots.
- `library.ts`: zero diagnostics anywhere under `src/library/**` in
  `default-importability-package.json`, even though every fixture file there
  imports a nominally package-private-by-default export — matches all of
  `library.ts`'s `toMatchInlineSnapshot(\`Array []\`)` assertions.

## Regenerating after a reference plugin change

Bump `EXPECTED_PLUGIN_VERSION` in `oracle/generate-snapshots.mjs`, re-run the
script, and re-run the spot-checks above by hand (or add them as an automated
step) before trusting the new snapshots.
