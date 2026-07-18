# @import-lint/cli

ImportLint enforces directory-level encapsulation in TypeScript and
JavaScript: tag an export `@package` in its JSDoc, and only files in that
directory (or nested below it) — its "package" — can import it; ImportLint flags every
import that breaks the rule. It's a small, fast Rust CLI, so it runs
without a TypeScript compiler or ESLint, and stays fast on large codebases
(it's a drop-in replacement for
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access)).

See the [main repository README](https://github.com/uhyo/import-lint#readme)
for the full pitch, a concrete example, and the
[guides](https://github.com/uhyo/import-lint/tree/master/docs/guides) —
this page covers just installing and running this package.

This package installs the `import-lint` CLI. It's a thin launcher that execs a
prebuilt native binary for your platform (installed automatically via
`optionalDependencies` — no compilation, no postinstall download scripts).

## Install

```sh
npm install -D @import-lint/cli
```

The package is named `@import-lint/cli`, but the installed command is still
`import-lint` (see Quickstart below).

## Quickstart

```sh
# Scaffold a .importlintrc.jsonc.
npx import-lint init

# Lint the current directory (or your config's `include` roots).
npx import-lint

# ESLint-compatible JSON output, for CI tooling.
npx import-lint --format json
```

With no config file, ImportLint lints `.` with the `package-access` rule at `error`
severity and defaults matching `eslint-plugin-import-access`.

## Use in CI

Exit code `1` on any error-severity diagnostic fails the build on violations;
`--format github` produces inline PR annotations on GitHub Actions:

```sh
npx import-lint --format github
```

See [Using in CI](https://github.com/uhyo/import-lint#using-in-ci) in the main
repository README for a full workflow example — also see there for the full
CLI reference, config file format, output formats, watch mode, and the
migration guide from `eslint-plugin-import-access`.

## Editor integration

The [ImportLint VS Code extension](https://marketplace.visualstudio.com/items?itemName=uhyo.import-lint)
shows violations as you type, on top of this package's binary — no separate
install needed. The binary this package installs also includes `import-lint
lsp`, for wiring up any other LSP-capable editor. See
[Editor integration](https://github.com/uhyo/import-lint#editor-integration)
in the main repository README for setup details.

## Supported platforms

`darwin-arm64`, `darwin-x64`, `linux-x64-gnu`, `linux-x64-musl`,
`linux-arm64-gnu`, `win32-x64`. On an unsupported platform (or with
`--omit=optional` installs), the CLI fails at run time with an actionable
error rather than breaking `npm install` for your whole project — see
[`cargo install import-lint`](https://crates.io/crates/import-lint) or
[GitHub Releases](https://github.com/uhyo/import-lint/releases) as fallbacks.

## License

MIT
