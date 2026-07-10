//! `import_lint` is the core library for ImportLint, a Rust reimplementation of
//! [`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access)
//! built on the [oxc](https://oxc.rs) toolchain.
//!
//! This crate hosts extraction (parsing a file into an owned module summary), the
//! module graph and resolver integration, and the rule engine. See `docs/PLAN.md` in
//! the workspace root for the full design.

pub mod extract;

pub use extract::{
    Access, CheckedEntry, EntryKind, ExportInfo, FileModuleInfo, extract as extract_file,
};
