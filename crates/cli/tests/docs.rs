//! `import-lint docs` / `import-lint explain` end-to-end tests: spawn the real
//! binary, mirroring `tests/init.rs`'s pattern. Neither subcommand reads the
//! filesystem or config, so no fixture directory is needed.

use std::process::{Command, ExitStatus};

struct Output {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_import-lint"))
        .args(args)
        .output()
        .expect("spawn import-lint");
    Output {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn docs_without_topic_lists_topics() {
    let out = run(&["docs"]);
    assert!(out.status.success(), "stderr: {}", out.stderr);
    for name in ["concepts", "config", "fixing"] {
        assert!(out.stdout.contains(name), "stdout: {}", out.stdout);
    }
}

#[test]
fn docs_prints_each_topic() {
    for (name, marker) in [
        ("concepts", "/** @package */"),
        ("config", "\"defaultImportability\": \"public\""),
        ("fixing", "index loophole"),
    ] {
        let out = run(&["docs", name]);
        assert!(out.status.success(), "stderr: {}", out.stderr);
        assert!(
            out.stdout.contains(marker),
            "docs {name} stdout: {}",
            out.stdout
        );
    }
}

#[test]
fn docs_unknown_topic_exits_2_and_lists_topics() {
    let out = run(&["docs", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, "");
    assert!(out.stderr.contains("unknown docs topic 'nope'"));
    assert!(out.stderr.contains("concepts, config, fixing"));
}

#[test]
fn explain_without_id_lists_message_ids() {
    let out = run(&["explain"]);
    assert!(out.status.success(), "stderr: {}", out.stderr);
    for id in [
        "package",
        "package:reexport",
        "private",
        "private:reexport",
        "unresolved",
    ] {
        assert!(out.stdout.contains(id), "stdout: {}", out.stdout);
    }
}

/// Each explanation leads with its id and quotes the diagnostic's message
/// text, so an agent that greps a lint run's output lands in the right place.
#[test]
fn explain_prints_each_message_id() {
    for (id, message) in [
        ("package", "Cannot import a package-private export"),
        (
            "package:reexport",
            "Cannot re-export a package-private export",
        ),
        ("private", "Cannot import a private export"),
        ("private:reexport", "Cannot re-export a private export"),
        ("unresolved", "Unresolved import specifier"),
    ] {
        let out = run(&["explain", id]);
        assert!(out.status.success(), "stderr: {}", out.stderr);
        assert!(out.stdout.starts_with(id), "stdout: {}", out.stdout);
        assert!(
            out.stdout.contains(message),
            "explain {id} stdout: {}",
            out.stdout
        );
    }
}

#[test]
fn explain_unknown_id_exits_2_and_lists_ids() {
    let out = run(&["explain", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, "");
    assert!(out.stderr.contains("unknown message id 'nope'"));
    assert!(out.stderr.contains("private:reexport"));
}
