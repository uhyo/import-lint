# ImportLint

A standalone Rust CLI linter for JavaScript and TypeScript that checks module-boundary
import access — the same functionality as
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access),
without depending on the TypeScript compiler or ESLint.

It reads JSDoc `@public`/`@package`/`@private` tags on your exports and flags imports
that cross a boundary they shouldn't: a `@private` export can only be imported from
its own file's package (directory, by default); a `@package` export can only be
imported from within the same "package" as the file that exports it.

Built on the [oxc](https://oxc.rs) toolchain for parsing and module resolution, so it
stays fast on large codebases without needing a full TypeScript type-check.

## Installation

```sh
cargo install --path . --locked
```

(Not yet published to crates.io; prebuilt binaries via GitHub Releases are planned —
see [Roadmap](#roadmap).)

## Quickstart

```sh
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

Two debug subcommands are also available (not part of the stable output contract):
`import-lint inspect <file>` dumps one file's extracted module info as JSON;
`import-lint graph [paths...]` dumps the discovery+resolution graph as JSON.

Flag resolution order is **CLI flag > config file > built-in default**. Rule options
(`indexLoophole`, `defaultImportability`, etc.) are configured only through the
config file — see below.

## Config file

ImportLint looks for `.importlintrc.jsonc` (or `.importlintrc.json`, if no `.jsonc`
file exists in the same directory) starting at the current directory and walking
upward to the filesystem root, unless `--config` names an explicit file. **The
directory containing the config file becomes the project root**: `include`,
`exclude`, and `tsconfig` are all resolved relative to it. With no config file
found, ImportLint uses the defaults below with the project root set to the current
directory.

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
  an `include` root) re-checks the whole project — cheaply, since only the changed
  file(s) are re-parsed (everything else is served from an extraction cache).
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

## Migration from eslint-plugin-import-access

ImportLint's `jsdoc` rule is a behavioral port of the plugin's `import-access/jsdoc`
rule, intended as a drop-in replacement:

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
  under the `docs/PLAN.md` §8 targets of 2 s / 4 s.
- **~155x faster than the reference `eslint-plugin-import-access`** on the
  same 5,000-file tree (157 ms vs. 24.4 s), reflecting that ImportLint parses
  once with oxc rather than running full TypeScript-type-aware ESLint.
- Watch-mode single-edit cycles at 10,000 files currently run **~155-220 ms**,
  short of the < 100 ms target — a known, documented gap (full re-check every
  cycle by design, per `crates/cli/src/watch.rs`'s M6 brief), not a regression;
  see `docs/benchmarks.md` for the phase-by-phase breakdown and follow-up plan.

Reproduce with `scripts/bench.sh` (add `--compare-eslint` for the ESLint
comparison) and `cargo bench -p import_lint --bench extract`.

## Roadmap

- Prebuilt binaries and a crates.io release.
