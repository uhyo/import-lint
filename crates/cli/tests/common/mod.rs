//! Shared fixture helpers for `tests/watch.rs` and `tests/lsp.rs` (L2, `docs/PLAN.md`
//! §5): both drive the same `WatchSession` engine (directly, or wrapped by the LSP
//! server), so they share the same "write files + build default session options"
//! plumbing. Pulled out of `tests/watch.rs` verbatim (no behavior change) when the L2
//! milestone added `tests/lsp.rs`.
//!
//! Not every helper is used by every test binary that includes this module, hence
//! the blanket allow rather than per-item annotations.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use import_lint_cli::watch::WatchSessionOptions;

pub fn write(dir: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
    path
}

pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|err| panic!("canonicalize {}: {err}", path.display()))
}

/// Default watch-session options: lint `dir` itself (no `--config`/`--tsconfig`
/// override, no `--report-unresolved`/`--quiet`) — config discovery still applies,
/// so a `.importlintrc.jsonc` written into `dir` before `WatchSession::new` is
/// picked up, matching non-watch `lint()`'s behavior.
pub fn session_options(dir: &Path) -> WatchSessionOptions {
    WatchSessionOptions {
        cli_paths: Vec::new(),
        explicit_config: None,
        cli_tsconfig: None,
        report_unresolved: false,
        quiet: false,
        cwd: dir.to_path_buf(),
    }
}

/// A single `@package` violation: `src/consumer.ts` imports `helper` from
/// `src/internal/util.ts`, which the default options forbid (sibling directories,
/// not ancestor/descendant — mirrors `crates/cli/tests/cli.rs`'s fixture).
pub fn write_violation_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let consumer = write(
        dir,
        "src/consumer.ts",
        "import { helper } from \"./internal/util\";\nconsole.log(helper);\n",
    );
    let util = write(
        dir,
        "src/internal/util.ts",
        "/** @package */\nexport const helper = 1;\n",
    );
    (canonical(&consumer), canonical(&util))
}
