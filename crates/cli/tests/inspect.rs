//! Integration tests for `import-lint inspect`, spawning the built binary directly
//! (`CARGO_BIN_EXE_import-lint`, set automatically by cargo for integration tests).

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_import-lint"))
}

#[test]
fn inspect_prints_extraction_as_json() {
    let dir = tempdir();
    let file = dir.path().join("mod.ts");
    std::fs::write(
        &file,
        "/** @package */\nexport const x = 1;\nimport { y } from \"./other\";\n",
    )
    .unwrap();

    let output = bin()
        .arg("inspect")
        .arg(&file)
        .output()
        .expect("failed to run import-lint");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(json["export_table"]["x"]["access"], "package");
    assert_eq!(json["checked_entries"][0]["imported_name"], "y");
    assert_eq!(json["checked_entries"][0]["specifier"], "./other");
}

#[test]
fn inspect_nonexistent_file_exits_2() {
    let output = bin()
        .arg("inspect")
        .arg("/nonexistent/path/does-not-exist.ts")
        .output()
        .expect("run");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stderr.is_empty());
}

#[test]
fn inspect_parse_error_exits_2() {
    let dir = tempdir();
    let file = dir.path().join("broken.ts");
    // Unterminated string literal -> guaranteed parse error.
    std::fs::write(&file, "export const x = \"unterminated;\n").unwrap();

    let output = bin().arg("inspect").arg(&file).output().expect("run");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stderr.is_empty());
}

/// Minimal temp-dir helper (kept dependency-free rather than pulling in `tempfile`).
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempdir() -> TempDir {
    let mut dir = std::env::temp_dir();
    let unique = format!(
        "import-lint-cli-test-{}-{}",
        std::process::id(),
        unique_suffix()
    );
    dir.push(unique);
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn unique_suffix() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Not cryptographically unique, just enough to avoid collisions between tests
    // running in the same process at the same moment.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
