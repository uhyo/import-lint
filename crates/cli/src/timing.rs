//! Opt-in per-phase timing instrumentation (PLAN.md §8, M7): set
//! `IMPORT_LINT_TIMING=1` (any non-empty value) to print a `phase: Xms` line to
//! stderr for each instrumented phase of the pipeline/watch cycle. Zero cost when
//! unset beyond one `OnceLock` check per call — cheap enough to leave permanently
//! wired into `runner.rs`/`report.rs` rather than behind a cfg flag, since it's the
//! quickest way to see where a slow run/cycle is actually spending its time (used
//! to diagnose the watch-cycle timing target in `crates/cli/tests/watch.rs`).

use std::sync::OnceLock;
use std::time::Instant;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("IMPORT_LINT_TIMING").is_some_and(|value| !value.is_empty())
    })
}

/// Run `f`, and if `IMPORT_LINT_TIMING` is set, print `[timing] <label>: <ms> ms`
/// to stderr. Returns `f`'s result either way.
pub fn phase<T>(label: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    eprintln!(
        "[timing] {label}: {:.3} ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    result
}
