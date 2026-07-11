# import-lint-core

Core engine for [`import-lint`](https://crates.io/crates/import-lint): file extraction,
module graph construction, and the JSDoc `@public`/`@package`/`@private` import-access
rule engine, built on the [oxc](https://oxc.rs) toolchain.

This crate is the library half of the `import-lint` CLI and is not intended to be a
stable, general-purpose public API on its own — it's split out primarily to support
future embedding (e.g. an LSP server). Most users want the
[`import-lint`](https://crates.io/crates/import-lint) binary instead.

See the [repository](https://github.com/uhyo/import-lint) for documentation, the
config file format, and usage.
