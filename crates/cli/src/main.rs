//! ImportLint CLI entry point.

use std::path::PathBuf;

use clap::Parser;

/// A Rust CLI linter that checks module-boundary import access (JSDoc `@package`/`@private`).
#[derive(Parser, Debug)]
#[command(name = "import-lint", version, about, long_about = None)]
struct Cli {
    /// Paths to lint (overrides the configured include roots). Not yet implemented.
    paths: Vec<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    let _ = cli.paths;

    println!("import-lint: not yet implemented");
}
