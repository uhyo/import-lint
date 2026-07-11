//! Library surface for the `import-lint` CLI, split out from the binary
//! (`src/main.rs`) so integration tests can drive discovery and the pipeline
//! directly without spawning a subprocess (PLAN-v1.md M2).

pub mod output;
pub mod report;
pub mod runner;
pub mod setup;
pub mod source_type;
mod timing;
pub mod walk;
pub mod watch;

pub use runner::{RunnerOptions, run};
pub use source_type::{SUPPORTED_EXTENSIONS_MESSAGE, source_type_for_path};
pub use walk::{walk, walk_with_excludes};
