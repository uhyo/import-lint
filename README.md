# ImportLint

[![crates.io](https://img.shields.io/crates/v/import-lint.svg)](https://crates.io/crates/import-lint)
[![npm](https://img.shields.io/npm/v/%40import-lint%2Fcli.svg)](https://www.npmjs.com/package/@import-lint/cli)

ImportLint enforces directory-level encapsulation in TypeScript and
JavaScript: tag an export `@package` in its JSDoc, and only files in that
directory (or nested below it) — its "package" — can import it; ImportLint flags every
import that breaks the rule. It's a small, fast Rust CLI, so it runs
without a TypeScript compiler or ESLint, and stays fast on large codebases.

**Already migrating from `eslint-plugin-import-access`?** Jump straight to
[Migration](#migration-from-eslint-plugin-import-access) — this README
teaches ImportLint from scratch first.

## Why?

The largest unit of encapsulation TypeScript and JavaScript actually
enforce is the file: something you don't export is invisible outside it,
but the moment you export it, it's visible — and importable — from
anywhere in the project. There's no built-in way to say "this is shared
between these five files, but not the rest of the codebase."

That gap pushes real projects toward workarounds nobody's actually
enforcing: oversized files (to keep more logic behind the one boundary that
does exist), `_internal`-style naming conventions a code review has to
catch by eye, or index files that gesture at a public surface without
anyone checking that the rest of the module respects it. None of these get
more reliable as a project grows — they get less.

ImportLint adds a directory-level layer on top of the file: tag an export
`@package` and it's importable only from its own directory and directories
nested inside it (or a boundary you name explicitly — see
[`packageDirectory`](#config-file) below); tag it `@private` and it's not
importable from anywhere outside its own file, not even the rest of its
directory. The check needs neither a TypeScript program nor ESLint and runs
in milliseconds, so nothing stops you from enforcing it on every keystroke
or in CI.

## Example

```
src/
├── cart/
│   └── total.ts     ── computeTotal(), tagged @package
└── receipt.ts        ── imports computeTotal from outside cart/
```

`src/cart/total.ts`:

```ts
/** @package */
export function computeTotal(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}
```

`src/receipt.ts`:

```ts
import { computeTotal } from "./cart/total";

console.log(computeTotal([1, 2, 3]));
```

No config file needed — this works with ImportLint's defaults:

```
$ import-lint .
src/receipt.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

The fix is one line: tag `computeTotal` `@public` instead, if it's meant to
be used from anywhere — or leave it `@package` and move `receipt.ts` inside
`cart/`, if it isn't. Either way, the next run is clean, with no output.

For the full mental model, see the
[Concepts guide](https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md);
for a longer walkthrough, see the
[Tutorial](https://github.com/uhyo/import-lint/blob/master/docs/guides/tutorial.md).

## Project Status

Treat this as a **beta product** until it reaches v1.0.0. This is Vibe Coded; this is supposed to work exactly like `eslint-plugin-import-access` which has already been proven useful in production. We believe it works and it's 100x faster than the ESLint plugin.

## Getting started

### Install

**npm** (recommended for JS/TS projects):

```sh
npm install -D @import-lint/cli
```

This installs the `import-lint` command — run it with `npx import-lint`.
Prebuilt native binaries for darwin-arm64, darwin-x64, linux-x64 (glibc & musl),
linux-arm64 (glibc), and win32-x64 are installed automatically via
`optionalDependencies`, so there's no compilation and no postinstall scripts.

**Cargo**:

```sh
cargo install import-lint
```

Or grab a prebuilt binary for your platform from
[GitHub Releases](https://github.com/uhyo/import-lint/releases).

To build from a local checkout instead:

```sh
cargo install --path crates/cli --locked
```

### Configure and run

```sh
# Installed via npm? Prefix these with npx, e.g. `npx import-lint`.

# Scaffold a .importlintrc.jsonc, interactively or via --preset.
import-lint init

# Lint the current directory (or your config's `include` roots).
import-lint

# Lint specific paths, overriding config `include`.
import-lint src lib

# ESLint-compatible JSON output, for CI tooling.
import-lint --format json
```

With no config file, ImportLint lints `.` with the `jsdoc` rule at `error` severity
and every option at its default (identical to `eslint-plugin-import-access`'s
defaults — see [Migration](#migration-from-eslint-plugin-import-access)).
`import-lint init` scaffolds a fully commented starting point instead of hand-writing
one, from one of three presets — see [Config file](#config-file) below, and the
[Adoption guide](https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md)
for which one fits your project and how to roll it out.

### Guides

[`docs/guides/`](https://github.com/uhyo/import-lint/tree/master/docs/guides)
has three short guides for the second sitting — this README stays a
complete reference on its own:

- [**Concepts**](https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md) —
  the mental model: importability, package directories, both loopholes,
  one-hop re-export semantics, external vs. internal.
- [**Tutorial**](https://github.com/uhyo/import-lint/blob/master/docs/guides/tutorial.md) —
  a ~10-minute walkthrough: create a boundary, hit a violation, fix it three
  different ways.
- [**Adoption**](https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md) —
  choosing a preset and rolling it out, including a phased strategy for an
  existing codebase.

## CLI flags

```
import-lint [paths...]
```

| Flag | Description | Default |
|---|---|---|
| `paths...` | Paths to lint. Overrides the config file's `include` when given. | config `include`, or `.` with no config |
| `--config <path>` | Explicit config file. Exits `2` if missing or invalid. | discovered by walking up from cwd |
| `--format <pretty\|json\|github>` | Output format — see [Output formats](#output-formats). | `pretty` |
| `--threads <n>` | Rayon thread pool size for parsing/resolving. | number of cores |
| `--tsconfig <path>` | Path to the project's `tsconfig.json`, for resolver `paths`/`baseUrl`. Overrides the config file. | config `tsconfig`, else `<project root>/tsconfig.json` if present |
| `--report-unresolved` | Emit a warning for every import specifier that fails to resolve, instead of skipping it silently. | off |
| `--quiet` | Suppress warning-severity output (errors only), like `eslint --quiet`. | off |
| `--watch` | Watch mode: re-lint on file changes — see [Watch mode](#watch-mode). | off |
| `--watch-poll [ms]` | Watch mode using a polling watcher. Implies `--watch`. | off |

`import-lint init [--preset <name>] [--force]` scaffolds `.importlintrc.jsonc`
into the current directory — see [Config file](#config-file). Two debug
subcommands are also available (not part of the stable output contract):
`import-lint inspect <file>` dumps one file's extracted module info as JSON;
`import-lint graph [paths...]` dumps the discovery+resolution graph as JSON.

Flag resolution order is **CLI flag > config file > built-in default**. Rule options
(`indexLoophole`, `defaultImportability`, etc.) are configured only through the
config file — see below.

## Config file

Run `import-lint init` to scaffold one instead of hand-writing it: interactively
(a numbered picker, if run in a terminal) or non-interactively via
`--preset <name>`. Three presets are available — `standard` (the `*.package`
naming convention: directories named `foo.package` are encapsulation boundaries;
recommended for new projects), `gradual` (annotation-driven: exports stay public
until tagged `@package`/`@private`; for adopting on an existing codebase), and
`monorepo` (boundaries at `packages/*`: no relative reach-ins across workspace
packages). A preset only picks starting values for the `jsdoc` options below — the
generated file is plain, fully editable config with no reference back to the preset.
See the [Adoption guide](https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md)
for a comparison and a rollout playbook per preset.

ImportLint looks for `.importlintrc.jsonc` (or `.importlintrc.json`, if no `.jsonc`
file exists in the same directory) starting at the current directory and walking
upward to the filesystem root, unless `--config` names an explicit file. **The
directory containing the config file becomes the project root**: `include`,
`exclude`, and `tsconfig` are all resolved relative to it. With no config file
found, ImportLint uses the defaults below with the project root set to the current
directory.

The options below are the levers behind the concepts explained in the
[Concepts guide](https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md) —
this is the quick reference; that guide is the "why", with a worked example per
option.

```jsonc
// .importlintrc.jsonc
{
  // Roots to walk for lint targets, relative to the project root.
  "include": ["."],

  // Extra glob patterns to skip, on top of .gitignore. Relative to the project root.
  "exclude": [],

  // Path to tsconfig.json (for resolver `paths`/`baseUrl`), relative to the
  // project root. Defaults to "<project root>/tsconfig.json" if it exists.
  // "tsconfig": "./tsconfig.json",

  "rules": {
    "jsdoc": {
      // "error" | "warn" | "off". An `off` rule is never checked.
      "severity": "error",

      // Below: identical options, names, and defaults to
      // eslint-plugin-import-access's `import-access/jsdoc` rule.

      // Treat a file named "index.{js,ts,jsx,tsx,mjs,cjs,...}" as if its parent
      // directory were the exporting file, for package-boundary purposes.
      "indexLoophole": true,

      // Treat "foo/bar.ts" as in-package with "foo.ts" (one directory level,
      // matching the importer's own filename stem).
      "filenameLoophole": false,

      // Access level assumed for an export with no recognized JSDoc access tag.
      // "public" | "package" | "private"
      "defaultImportability": "public",

      // How a bare specifier matching the importer's own package name is
      // classified. "external" (never checked) | "internal" (checked normally).
      "treatSelfReferenceAs": "external",

      // Glob patterns (matched against the exporting file's project-relative
      // path) that are never checked, regardless of access level.
      "excludeSourcePatterns": [],

      // Glob patterns identifying "package" directories (matched against both
      // basename and project-relative path). Unset: a file's own containing
      // directory is its package. A `!`-prefixed pattern excludes a directory
      // that would otherwise match.
      // "packageDirectory": ["packages/*"],
    }
  }
}
```

Unknown keys anywhere in the config file (a typo'd option name, an unrecognized
rule) are a hard load error (exit `2`) rather than a silently ignored no-op.

## Output formats

- **`pretty`** (default) — ESLint-stylish-like, grouped by file, paths relative to
  the current directory. Colored when stdout is a TTY, plain otherwise. Prints
  nothing for a clean run.

  ```
  src/foo/bar.ts
    3:10  error    Cannot import a package-private export 'x'  import-access/jsdoc
    5:1   warning  Unresolved import specifier './gone'         import-access/unresolved

  ✖ 2 problems (1 error, 1 warning)
  ```

- **`json`** — a single-line, ESLint-compatible JSON array: one entry per linted
  file (including clean files, with an empty `messages` array — matching ESLint's
  own behavior), each with `filePath`, `messages` (`ruleId`, `severity` (`2` =
  error, `1` = warning), `message`, `messageId`, `line`, `column`, `endLine`,
  `endColumn`), `errorCount`, `warningCount`, and `fixableErrorCount` /
  `fixableWarningCount` (always `0` — ImportLint has no autofixes). Suitable for
  any tool that already consumes `eslint --format json`.

- **`github`** — one [GitHub Actions workflow command](https://docs.github.com/en/actions/using-workflows/workflow-commands-for-github-actions)
  per diagnostic:

  ```
  ::error file=src/a.ts,line=3,col=10,endLine=3,endColumn=20::Cannot import a package-private export 'x'
  ```

## Using in CI

Exit code `1` on any error-severity diagnostic makes ImportLint CI-friendly out of
the box, and `--format github` emits GitHub Actions workflow commands so
violations show up as inline annotations on the PR.

```yaml
jobs:
  import-lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npm ci
      - run: npx import-lint --format github
```

For machine-readable output (e.g. to feed other tools), use `--format json`,
which is ESLint-compatible.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | No error-severity diagnostics (a clean run, or only warnings). |
| `1` | At least one error-severity diagnostic. |
| `2` | Invalid usage, an invalid/missing `--config` file, or an internal error. |

`--report-unresolved` diagnostics and any diagnostic under a rule configured
`"severity": "warn"` are warnings — they're included in output (unless `--quiet`)
but never affect the exit code.

## Watch mode

```sh
import-lint --watch
```

Runs an initial lint, then keeps re-linting as files change until you kill the
process (Ctrl-C). Each cycle re-prints the full diagnostic list — on a TTY with the
default `pretty` format the screen is cleared first; piping/redirecting output (or
`--format json`/`--format github`) just appends each cycle's full output, so
`import-lint --watch --format json | tee log.jsonl` produces a readable transcript.
A status line follows every re-render:

```
✖ 1 problem (1 error, 0 warnings) — rechecked 42 files in 8 ms (watching, Ctrl-C to exit)
```

**What triggers a re-run:**

- Editing a file already picked up by the lint (a `.ts`/`.tsx`/`.js`/... file under
  an `include` root) re-parses just that file and re-checks only the files whose
  diagnostics could actually be affected: the file itself, and — only if the edit
  changed what it exports — its importers, followed transitively through
  `export * from` chains. Everything else is untouched.
- Adding, removing, or renaming any file, or editing any `package.json`, re-walks
  the project and rebuilds the resolver from scratch (a new or deleted file can
  shadow a specifier resolution elsewhere).
- Editing the config file (`.importlintrc.jsonc`/`.json`) or the `tsconfig.json`
  reloads it and rebuilds everything. **If the edited config is invalid, the
  previous config keeps running** (the error is reported, but watch mode never
  exits on a bad config edit) — fix it and save again.

**`--watch-poll [interval-ms]`** (default interval `500`) uses a polling watcher
instead of the platform-recommended one (inotify on Linux). Use this:

- **On WSL2, when editing files from the Windows side** (e.g. VS Code running on
  Windows against a `\\wsl$\...` or `/mnt/c/...` path) — inotify does not reliably
  see writes that originate outside the Linux VM (see
  `docs/research/spike-s5-watch-wsl2.md`).
- **On network filesystems** (NFS, Samba, and similar), where inotify support is
  generally unreliable or absent.

**Limitation:** `node_modules` is never watched (matching discovery, which never
walks into it) — a `node_modules` change never triggers a re-run. Reinstalling
dependencies or editing a linked/workspace package under `node_modules` requires
restarting `import-lint --watch` manually.

## Editor integration

The [ImportLint VS Code extension](https://marketplace.visualstudio.com/items?itemName=uhyo.import-lint)
(`uhyo.import-lint`, also on [Open VSX](https://open-vsx.org/extension/uhyo/import-lint))
shows violations as you type, including cross-file ones — a change in a file
you haven't opened can surface a diagnostic in a file that imports it. It
needs no extra install beyond `npm install -D @import-lint/cli`: the
extension finds the workspace binary automatically (`importLint.binaryPath`
overrides this, and `PATH` is the fallback). It activates automatically when
an `.importlintrc.json(c)` is present (`importLint.enabled` forces it on or
off), and `importLint.run` controls whether it lints on every keystroke
(`onType`, the default) or only on save (`onSave`).

**Other editors:** the same binary speaks [LSP](https://microsoft.github.io/language-server-protocol/)
over stdio via `import-lint lsp`. For Neovim (0.11+, using `vim.lsp.config`/
`vim.lsp.enable`):

```lua
vim.lsp.config('import_lint', {
  cmd = { 'import-lint', 'lsp' },
  filetypes = { 'javascript', 'javascriptreact', 'typescript', 'typescriptreact' },
  root_markers = { '.importlintrc.jsonc', '.importlintrc.json', '.git' },
})
vim.lsp.enable('import_lint')
```

## Migration from eslint-plugin-import-access

ImportLint's `jsdoc` rule is a behavioral port of the plugin's `import-access/jsdoc`
rule, intended as a drop-in replacement:

- **Swap the package**: in `package.json` `devDependencies`, replace
  `eslint-plugin-import-access` with `@import-lint/cli`
  (`npm uninstall eslint-plugin-import-access && npm install -D @import-lint/cli`).
  The installed command is `import-lint`; remove the plugin from your ESLint
  config.
- **Options map 1:1, name-for-name**, with the same defaults: `indexLoophole`,
  `filenameLoophole`, `defaultImportability`, `treatSelfReferenceAs`,
  `excludeSourcePatterns`, `packageDirectory`. Copy your rule options straight into
  `rules.jsdoc` in `.importlintrc.jsonc`.
- The `json` output format's `ruleId` is `import-access/jsdoc`, matching the
  plugin's own rule ID, so existing ESLint-output consumers (CI parsers, reviewdog,
  etc.) need no changes.
- Same one-hop re-export semantics: only the directly-imported file's own JSDoc (or
  its `export * from` chain) governs a re-export's importability — never a second
  hop through what *that* file re-exports.
- **Unresolvable imports are skipped silently**, matching the plugin's behavior
  (which would have failed type-checking earlier via TypeScript). Pass
  `--report-unresolved` to see them as warnings instead.
- There's no `no-program` diagnostic — that check existed only because the plugin
  needed a TypeScript program; ImportLint doesn't use one.
- Same blind spots as the plugin, on purpose: `export * from` statements are
  followed for the check, but `import * as ns` namespace access, dynamic
  `import()`, `import x = require()`, and CommonJS `require()` are not checked.

## Performance

Measured on a 16-core AMD Ryzen 7 PRO 6850U laptop running **WSL2** (treat as
directional, not absolute — see `docs/benchmarks.md` for the full methodology,
machine details, and reproduction commands):

- **Cold lint: ~157 ms for 5,000 files, ~323 ms for 10,000 files** — well
  under the `docs/PLAN-v1.md` §8 targets of 2 s / 4 s.
- **~155x faster than the reference `eslint-plugin-import-access`** on the
  same 5,000-file tree (157 ms vs. 24.4 s), reflecting that ImportLint parses
  once with oxc rather than running full TypeScript-type-aware ESLint.
- **Watch-mode single-edit cycles at 10,000 files run ~5 ms**, well under the
  < 100 ms target — an incremental fast path (PLAN-v1.md §7) patches the module
  graph and re-checks only the dirty subset instead of the whole project; see
  `docs/benchmarks.md` for the design and measurements.

Reproduce with `scripts/bench.sh` (add `--compare-eslint` for the ESLint
comparison) and `cargo bench -p import-lint-core --bench extract`.

## Roadmap

- v1.0 stabilization following the v0.1.0 crates.io / GitHub Releases launch.
