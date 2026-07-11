//! `gen-fixture <out-dir> --files N [--seed S]`: generates a deterministic synthetic
//! TypeScript project at `<out-dir>` for `import-lint`'s benchmarks (PLAN-v1.md §8, M7).
//! See `crates/gen-fixture/src/lib.rs` for the shape it produces. Hand-rolled arg
//! parsing (no `clap`): this crate is intentionally dependency-free.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use gen_fixture::{GenOptions, generate};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut out_dir: Option<PathBuf> = None;
    let mut files: Option<usize> = None;
    let mut seed: u64 = 42;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--files" => {
                let Some(value) = args.get(i + 1) else {
                    return usage_error("--files requires a value");
                };
                files = match value.parse() {
                    Ok(n) => Some(n),
                    Err(_) => return usage_error(&format!("invalid --files value: {value}")),
                };
                i += 2;
            }
            "--seed" => {
                let Some(value) = args.get(i + 1) else {
                    return usage_error("--seed requires a value");
                };
                seed = match value.parse() {
                    Ok(s) => s,
                    Err(_) => return usage_error(&format!("invalid --seed value: {value}")),
                };
                i += 2;
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other if out_dir.is_none() => {
                out_dir = Some(PathBuf::from(other));
                i += 1;
            }
            other => return usage_error(&format!("unexpected argument: {other}")),
        }
    }

    let Some(out_dir) = out_dir else {
        return usage_error("missing <out-dir>");
    };
    let Some(files) = files else {
        return usage_error("missing --files N");
    };

    let start = Instant::now();
    match generate(&out_dir, &GenOptions { files, seed }) {
        Ok(result) => {
            println!(
                "gen-fixture: wrote {} files ({} content, {} barrels, {} ambient) to {} in {:.2}s",
                result.total_files(),
                result.content_files,
                result.barrel_files,
                result.ambient_files,
                out_dir.display(),
                start.elapsed().as_secs_f64(),
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!(
                "gen-fixture: failed to generate {}: {err}",
                out_dir.display()
            );
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!("Usage: gen-fixture <out-dir> --files N [--seed S]");
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("gen-fixture: {message}");
    print_usage();
    ExitCode::from(2)
}
