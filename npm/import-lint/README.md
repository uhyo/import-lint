# import-lint

A fast standalone linter enforcing JSDoc `@public`/`@package`/`@private` import
access rules in TypeScript/JavaScript projects — a drop-in replacement for
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access)
that doesn't need ESLint or a TypeScript type-check to run.

This package installs the `import-lint` CLI. It's a thin launcher that execs a
prebuilt native binary for your platform (installed automatically via
`optionalDependencies` — no compilation, no postinstall download scripts).

## Install

```sh
npm install -D import-lint
```

## Quickstart

```sh
# Lint the current directory (or your config's `include` roots).
npx import-lint

# ESLint-compatible JSON output, for CI tooling.
npx import-lint --format json
```

With no config file, ImportLint lints `.` with the `jsdoc` rule at `error`
severity and defaults matching `eslint-plugin-import-access`.

See the [main repository README](https://github.com/uhyo/import-lint#readme)
for the full CLI reference, config file format, output formats, watch mode,
and the migration guide from `eslint-plugin-import-access`.

## Supported platforms

`darwin-arm64`, `darwin-x64`, `linux-x64-gnu`, `linux-x64-musl`,
`linux-arm64-gnu`, `win32-x64`. On an unsupported platform (or with
`--omit=optional` installs), the CLI fails at run time with an actionable
error rather than breaking `npm install` for your whole project — see
[`cargo install import-lint`](https://crates.io/crates/import-lint) or
[GitHub Releases](https://github.com/uhyo/import-lint/releases) as fallbacks.

## License

MIT
