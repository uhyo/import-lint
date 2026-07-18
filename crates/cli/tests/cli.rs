//! End-to-end CLI tests (PLAN-v1.md §5–§6, M5): spawn the real `import-lint` binary
//! (`env!("CARGO_BIN_EXE_import-lint")`) against a fresh `TempDir` fixture per test
//! and assert on its exit code, stdout, and stderr — the parts of the M5 brief that
//! only exist at the binary boundary (config discovery, flag precedence, output
//! formats, exit codes) rather than in `import_lint_cli`'s library API (already
//! covered by `pipeline.rs` and `conformance.rs`).

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

/// A single `@package` violation: `src/consumer.ts` imports a `@package`-tagged
/// export from `src/internal/util.ts`, a sibling (not ancestor/descendant)
/// directory — a real cross-package-boundary violation under default options
/// (verified against `crates/core/src/rule/in_package.rs`'s
/// `importer_ancestor_of_exporter_package_fails` case).
fn write_violation_fixture(dir: &Path) {
    write(
        dir,
        "src/consumer.ts",
        "import { helper } from \"./internal/util\";\nconsole.log(helper);\n",
    );
    write(
        dir,
        "src/internal/util.ts",
        "/** @package */\nexport const helper = 1;\n",
    );
}

/// True if some line of `stdout` starts (after leading whitespace) with a
/// `line:column` pair and contains `severity` — i.e. a pretty-format diagnostic
/// line, without pinning down exact numbers.
fn has_location_line(stdout: &str, severity: &str) -> bool {
    stdout.lines().any(|line| {
        let trimmed = line.trim_start();
        let Some(loc) = trimmed.split_whitespace().next() else {
            return false;
        };
        let mut parts = loc.split(':');
        let (Some(l), Some(c), None) = (parts.next(), parts.next(), parts.next()) else {
            return false;
        };
        l.parse::<u32>().is_ok() && c.parse::<u32>().is_ok() && trimmed.contains(severity)
    })
}

#[test]
fn clean_project_exits_0_with_empty_stdout() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");

    let out = run_in(dir.path(), &[]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn package_violation_pretty_output_exits_1_with_no_ansi() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());

    let out = run_in(dir.path(), &[]);

    assert_eq!(out.status.code(), Some(1));
    // Pretty output renders paths with the native separator (cf. the
    // paths-override test below).
    assert!(out.stdout.contains("src/consumer.ts") || out.stdout.contains("src\\consumer.ts"));
    assert!(
        out.stdout
            .contains("Cannot import a package-private export 'helper'")
    );
    assert!(out.stdout.contains("package-access"));
    assert!(has_location_line(&out.stdout, "error"));
    assert!(!out.stdout.contains('\x1b'), "stdout: {}", out.stdout);
}

#[test]
fn severity_warn_exits_0_and_renders_as_warning() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "rules": { "package-access": { "severity": "warn" } } }"#,
    );

    let out = run_in(dir.path(), &[]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert!(has_location_line(&out.stdout, "warning"));
    assert!(
        out.stdout
            .contains("Cannot import a package-private export 'helper'")
    );
}

#[test]
fn severity_warn_with_quiet_suppresses_all_output() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "rules": { "package-access": { "severity": "warn" } } }"#,
    );

    let out = run_in(dir.path(), &["--quiet"]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn severity_off_exits_0_with_no_output() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "rules": { "package-access": { "severity": "off" } } }"#,
    );

    let out = run_in(dir.path(), &[]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn json_format_matches_eslint_shape() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    write(dir.path(), "src/clean.ts", "export const clean = 1;\n");

    let out = run_in(dir.path(), &["--format", "json"]);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", out.stderr);
    let value: serde_json::Value =
        serde_json::from_str(out.stdout.trim()).expect("stdout should be valid JSON");
    let entries = value.as_array().expect("top-level JSON value is an array");

    // Every file (including clean ones) gets an entry with an absolute filePath.
    assert!(entries.iter().all(|entry| {
        Path::new(entry["filePath"].as_str().expect("filePath is a string")).is_absolute()
    }));

    let clean_entry = entries
        .iter()
        .find(|entry| entry["filePath"].as_str().unwrap().ends_with("clean.ts"))
        .expect("clean.ts should have an entry even with no messages");
    assert_eq!(clean_entry["messages"].as_array().unwrap().len(), 0);
    assert_eq!(clean_entry["errorCount"], 0);
    assert_eq!(clean_entry["fixableErrorCount"], 0);
    assert_eq!(clean_entry["fixableWarningCount"], 0);

    let consumer_entry = entries
        .iter()
        .find(|entry| entry["filePath"].as_str().unwrap().ends_with("consumer.ts"))
        .expect("consumer.ts should have the violation");
    assert_eq!(consumer_entry["errorCount"], 1);
    assert_eq!(consumer_entry["warningCount"], 0);
    let message = &consumer_entry["messages"][0];
    assert_eq!(message["ruleId"], "package-access");
    assert_eq!(message["severity"], 2);
    assert_eq!(message["messageId"], "package");
}

#[test]
fn github_format_produces_error_workflow_command() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());

    let out = run_in(dir.path(), &["--format", "github"]);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", out.stderr);
    assert!(
        out.stdout
            .lines()
            .any(|line| { line.starts_with("::error file=") && line.contains("src/consumer.ts") })
    );
}

#[test]
fn config_discovery_walks_up_and_include_limits_lint_targets() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "include": ["src"] }"#,
    );
    write_violation_fixture(dir.path());
    // A second violation outside `include`, which must NOT be linted.
    write(
        dir.path(),
        "other/consumer.ts",
        "import { helper } from \"./internal/util\";\nconsole.log(helper);\n",
    );
    write(
        dir.path(),
        "other/internal/util.ts",
        "/** @package */\nexport const helper = 1;\n",
    );

    let subdir = dir.path().join("tools/nested");
    fs::create_dir_all(&subdir).unwrap();

    let out = run_in(&subdir, &[]);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", out.stderr);
    assert!(out.stdout.contains("src/consumer.ts") || out.stdout.contains("src\\consumer.ts"));
    assert!(
        !out.stdout.contains("other/consumer.ts") && !out.stdout.contains("other\\consumer.ts")
    );
}

#[test]
fn exclude_glob_excludes_the_violating_file() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "exclude": ["src/consumer.ts"] }"#,
    );

    let out = run_in(dir.path(), &[]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn unknown_config_key_exits_2_and_mentions_key_and_file() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "includ": ["src"] }"#,
    );

    let out = run_in(dir.path(), &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.contains("includ"), "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains(".importlintrc.jsonc"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn jsdoc_rule_key_exits_2_and_prints_the_rename_hint() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        ".importlintrc.jsonc",
        r#"{ "rules": { "jsdoc": { "severity": "warn" } } }"#,
    );

    let out = run_in(dir.path(), &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stderr
            .contains("the rule \"jsdoc\" was renamed to \"package-access\"; update your config"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn report_unresolved_emits_warning_and_exits_0() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "src/a.ts",
        "import { x } from \"./missing\";\nconsole.log(x);\n",
    );

    let out = run_in(dir.path(), &["--report-unresolved"]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert!(has_location_line(&out.stdout, "warning"));
    assert!(
        out.stdout
            .contains("Unresolved import specifier './missing'")
    );
    assert!(out.stdout.contains("import-access/unresolved"));
}

#[test]
fn report_unresolved_without_the_flag_stays_silent() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "src/a.ts",
        "import { x } from \"./missing\";\nconsole.log(x);\n",
    );

    let out = run_in(dir.path(), &[]);

    assert!(out.status.success(), "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn nonexistent_explicit_config_exits_2() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");

    let out = run_in(dir.path(), &["--config", "does-not-exist.jsonc"]);

    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stderr.contains("does-not-exist.jsonc"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn invalid_config_json_exits_2_and_mentions_file() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), ".importlintrc.jsonc", "{ not valid json ");

    let out = run_in(dir.path(), &[]);

    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stderr.contains(".importlintrc.jsonc"),
        "stderr: {}",
        out.stderr
    );
}

/// `--format` rejects an unrecognized value with clap's own usage-error exit code.
#[test]
fn invalid_format_value_exits_2() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/a.ts", "export const a = 1;\n");

    let out = run_in(dir.path(), &["--format", "yaml"]);

    assert_eq!(out.status.code(), Some(2));
}

/// `--config` pointing at an explicit path takes priority over discovery, and the
/// explicit config's directory becomes the project root (so its `include` is
/// resolved relative to it, not to `cwd`).
#[test]
fn explicit_config_flag_is_used_over_discovery() {
    let dir = TempDir::new().unwrap();
    write_violation_fixture(dir.path());
    let config_dir = dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();
    write(&config_dir, "custom.jsonc", r#"{ "include": ["../src"] }"#);

    let out = run_in(dir.path(), &["--config", "config/custom.jsonc"]);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", out.stderr);
    assert!(out.stdout.contains("consumer.ts"));
}
