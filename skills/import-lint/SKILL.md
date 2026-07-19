---
name: import-lint
description: Run ImportLint and fix its errors. Use when the project contains a .importlintrc.jsonc or .importlintrc.json file, when a lint run reports "Cannot import a package-private export" or "Cannot import a private export" (rule package-access), or when the user asks about package boundaries, import access, or @public/@package/@private annotations.
---

# ImportLint

ImportLint enforces directory-level encapsulation in TypeScript/JavaScript. A directory is a
"package" (unrelated to npm packages); its exports are importable only from files inside it until
an export is explicitly opened up with a JSDoc tag. It is a fast Rust CLI, a drop-in replacement
for eslint-plugin-import-access.

## Mental model

- Each export has an **importability**: `public` (importable anywhere), `package` (only within the
  same package), or `private` (nowhere, not even same-package). It is declared with a JSDoc tag
  directly above the `export`; untagged exports fall back to the config's `defaultImportability`.
- Child directories may import from ancestor packages; the reverse is a violation.
- If the config sets `packageDirectory` (glob patterns, e.g. `["**/*.package"]`), only matching
  directories are boundaries. Otherwise every directory is its own package.
- **Index loophole** (on by default): a bare re-export in a package's `index.ts` promotes that
  export one level out, to the parent package. This is the idiomatic way to expose a package's API.
- **One-hop re-export semantics**: a re-export statement's own JSDoc tag governs visibility for
  whoever imports through it. A bare (untagged) re-export resets importability to
  `defaultImportability` — even if the original export was `@public`.
- Only imports resolving to files inside the project are checked; npm packages and Node builtins
  are never flagged.

## Annotation syntax

Place directly above the `export` (case-sensitive tag names):

```ts
/** @public */
export const token = ...;   // also: @package, @private
/** @access public */        // equivalent alternate spelling
```

## Running the linter

```sh
npx import-lint              # lint paths from config `include` (or . without config)
npx import-lint src/ lib/    # lint specific paths (overrides `include`)
npx import-lint --format json   # ESLint-compatible JSON (machine-readable)
```

(If installed via cargo or a prebuilt binary, the command is `import-lint` without `npx`.)

- Exit codes: `0` clean or warnings only, `1` at least one error, `2` invalid usage/config.
- The default `pretty` format prints nothing when clean.
- Other useful flags: `--config <path>`, `--quiet` (errors only), `--report-unresolved`
  (warn on unresolved import specifiers), `--watch`, `--format github` (CI annotations).
- Config: `.importlintrc.jsonc` (or `.json`), discovered by walking up from cwd; its directory is
  the project root. Unknown config keys are a hard error (exit 2), not ignored.

## Diagnostics

All under rule `package-access`:

| Message | Meaning |
|---|---|
| `Cannot import a package-private export 'X'` | Importing a `package`-level export from outside its package |
| `Cannot re-export a package-private export 'X'` | A re-export statement reaches into another package's `package`-level export |
| `Cannot import a private export 'X'` | Importing a `private` export (never importable) |
| `Cannot re-export a private export 'X'` | A re-export statement reaches into a `private` export |

## Fixing a violation

First locate the exporting file and its package boundary (the nearest ancestor directory matching
`packageDirectory`, or the file's own directory if `packageDirectory` is unset). Then pick the
first fix that fits, in this order:

1. **Move the importing file into the package** — if it conceptually belongs there, no visibility
   change is needed at all.
2. **Re-export through the package's `index.ts`** (uses the index loophole) — add a bare
   re-export, e.g. `export { issueToken } from "./token";`. This exposes the export only one level
   out, to the parent package. To widen further, tag the re-export line itself
   (`/** @public */ export { issueToken } from "./token";`).
3. **Tag the original export `/** @public */`** — makes it importable from anywhere. This is
   rarely the right choice; prefer 1 or 2 so the boundary stays meaningful.

Do NOT fix violations by setting the rule severity to `"off"`/`"warn"`, adding
`excludeSourcePatterns`, or loosening `defaultImportability` unless the user explicitly asks —
those weaken checking project-wide rather than fixing the design issue.

After editing, rerun `npx import-lint` to confirm the error is gone and no new ones appeared.

## Common pitfalls

- A bare re-export does not preserve `@public` from the original export (one-hop rule); tag the
  re-export statement itself if it must stay public.
- `defaultImportability` defaults to `"public"` for backward compatibility, but projects
  scaffolded with `import-lint init` use `"package"` — check the config before assuming untagged
  exports are open.
- The tag must be in a JSDoc block (`/** ... */`) immediately above the export; line comments and
  detached comments are not recognized.

## Deeper documentation

The CLI ships built-in docs that always match the installed version — prefer these:

- `npx import-lint explain <message-id>` — what one diagnostic means and how to fix it.
  Message ids: `package`, `package:reexport`, `private`, `private:reexport`, `unresolved`
  (reported in the `messageId` field of `--format json` output).
- `npx import-lint docs <topic>` — condensed topic guides: `concepts` (mental model),
  `config` (all options and defaults), `fixing` (fix procedure).

Long-form guides (online):

- Concepts (full mental model): https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md
- Tutorial: https://github.com/uhyo/import-lint/blob/master/docs/guides/tutorial.md
- Gradual adoption strategies: https://github.com/uhyo/import-lint/blob/master/docs/guides/adoption.md
- All config options: https://github.com/uhyo/import-lint/blob/master/README.md
