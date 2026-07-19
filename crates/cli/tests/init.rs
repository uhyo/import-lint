//! `import-lint init` end-to-end tests (M9, `docs/PLAN-init.md` §5, milestone I1):
//! spawn the real binary (`env!("CARGO_BIN_EXE_import-lint")`) against a fresh
//! `TempDir` fixture per test, mirroring `tests/cli.rs`'s pattern.

use std::fs;
use std::path::Path;
use std::process::{Command, ExitStatus};

use tempfile::TempDir;

fn write(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
}

struct Output {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_import-lint"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn import-lint");
    Output {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// `import-lint init` writes `.importlintrc.jsonc` and prints nothing to
/// stdout (D-I7); a follow-up `import-lint` run against a trivial fixture then
/// exits clean using the generated config — the real exit criterion
/// (PLAN-init.md milestone I1). `Command::output()` never hands the child a
/// TTY, so this also proves `init` is fully non-interactive.
#[test]
fn init_scaffolds_a_config_that_lints_clean() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");

    let init_out = run_in(dir.path(), &["init"]);
    assert!(init_out.status.success(), "stderr: {}", init_out.stderr);
    assert_eq!(init_out.stdout, "", "stdout should be empty");
    assert!(
        dir.path().join(".importlintrc.jsonc").is_file(),
        "config file should exist"
    );

    let lint_out = run_in(dir.path(), &[]);
    assert!(
        lint_out.status.success(),
        "lint should succeed using the generated config, stderr: {}",
        lint_out.stderr
    );
    assert_eq!(lint_out.stdout, "");
}

/// The success output points at the built-in docs and the copy-pastable agent
/// skill (stderr only, per D-I7).
#[test]
fn init_success_output_mentions_docs_and_agent_skill() {
    let dir = TempDir::new().unwrap();

    let out = run_in(dir.path(), &["init"]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "", "stdout should be empty");
    assert!(
        out.stderr.contains("import-lint docs"),
        "stderr: {}",
        out.stderr
    );
    assert!(
        out.stderr
            .contains("https://github.com/uhyo/import-lint/tree/master/skills/import-lint"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn refuses_to_overwrite_an_existing_jsonc_config_without_force() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".importlintrc.jsonc", "{}");

    let out = run_in(dir.path(), &["init"]);

    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, "");
    assert!(
        out.stderr.contains(".importlintrc.jsonc"),
        "stderr: {}",
        out.stderr
    );
    // The pre-existing file must survive untouched.
    assert_eq!(
        fs::read_to_string(dir.path().join(".importlintrc.jsonc")).unwrap(),
        "{}"
    );
}

#[test]
fn refuses_to_overwrite_an_existing_json_config_without_force() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".importlintrc.json", "{}");

    let out = run_in(dir.path(), &["init"]);

    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stderr.contains(".importlintrc.json"),
        "stderr: {}",
        out.stderr
    );
    assert!(!dir.path().join(".importlintrc.jsonc").exists());
}

#[test]
fn force_overwrites_an_existing_jsonc_config() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".importlintrc.jsonc", "{ /* old */ }");

    let out = run_in(dir.path(), &["init", "--force"]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    let contents = fs::read_to_string(dir.path().join(".importlintrc.jsonc")).unwrap();
    assert!(
        contents.contains("\"packageDirectory\": [\"**/*.package\"]"),
        "contents: {contents}"
    );
}

#[test]
fn force_with_existing_json_notes_that_jsonc_shadows_it() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".importlintrc.json", "{}");

    let out = run_in(dir.path(), &["init", "--force"]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert!(dir.path().join(".importlintrc.jsonc").is_file());
    assert!(dir.path().join(".importlintrc.json").is_file());
    assert!(out.stderr.contains("shadows"), "stderr: {}", out.stderr);
}

#[test]
fn ancestor_config_gets_a_note_that_the_new_file_takes_over() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".importlintrc.jsonc", "{}");
    let nested = dir.path().join("nested");
    fs::create_dir_all(&nested).unwrap();

    let out = run_in(&nested, &["init"]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert!(nested.join(".importlintrc.jsonc").is_file());
    assert!(out.stderr.contains("takes over"), "stderr: {}", out.stderr);
}

/// `--preset` was removed (init always emits the one template); passing it is
/// now a clap usage error and must not write anything.
#[test]
fn removed_preset_flag_exits_2() {
    let dir = TempDir::new().unwrap();

    let out = run_in(dir.path(), &["init", "--preset", "standard"]);

    assert_eq!(out.status.code(), Some(2));
    assert!(!dir.path().join(".importlintrc.jsonc").exists());
}
