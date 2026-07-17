# ImportLint for VS Code

ImportLint enforces directory-level encapsulation in TypeScript and
JavaScript: tag an export `@package` in its JSDoc, and only files in that
directory (or nested below it) — its "package" — can import it; ImportLint flags every
import that breaks the rule. It's a small, fast Rust CLI, so it runs
without a TypeScript compiler or ESLint, and stays fast on large codebases
(it's a drop-in replacement for
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access)).

This extension shows real-time diagnostics for TypeScript and JavaScript,
powered by the `import-lint lsp` language server — including detecting
violations introduced in files you haven't even opened yet. See the
[main repository README](https://github.com/uhyo/import-lint#readme) and
[guides](https://github.com/uhyo/import-lint/tree/master/docs/guides) for
the full pitch and mental model.

## Requirements

This extension is a thin client: it does not bundle a linter binary. It looks
for one in this order:

1. The `importLint.binaryPath` setting, if set.
2. `@import-lint/cli` installed in the workspace's `node_modules`
   (`npm install -D @import-lint/cli`).
3. `import-lint` on your `PATH` (e.g. installed via `cargo install import-lint`).

The resolved binary must be **version 0.1.2 or later** (the first release with
the `lsp` subcommand); older binaries are detected and rejected with a clear
notification rather than failing silently.

## Settings

| Setting | Default | Description |
|---|---|---|
| `importLint.enabled` | `"auto"` | `"auto"` starts the server only when a `.importlintrc.json(c)` exists in the workspace; `"on"` always starts it; `"off"` never starts it. |
| `importLint.run` | `"onType"` | `"onType"` lints on every keystroke; `"onSave"` lints only on save. |
| `importLint.binaryPath` | `""` | Absolute path to an `import-lint` binary. Overrides automatic resolution. |
| `importLint.trace.server` | `"off"` | Standard `vscode-languageclient` trace level (`off` \| `messages` \| `verbose`) for debugging the LSP connection. |

## Commands

- **ImportLint: Restart Server** (`importLint.restart`) — stops and
  re-locates/restarts the server. Use this after `npm install`ing a different
  `@import-lint/cli` version, or after changing `importLint.binaryPath`.

## Learn more

See the [ImportLint repository](https://github.com/uhyo/import-lint) for
configuration (`.importlintrc.json`), rule documentation, and release notes.
