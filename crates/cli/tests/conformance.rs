//! Conformance suite (docs/PLAN-v1.md §9.1, M3 exit criterion): lint
//! `tests/conformance/fixtures/project/src` with ImportLint under each option set
//! captured in `tests/conformance/expected/manifest.json`, and diff the resulting
//! diagnostics against that option set's `expected/<name>.json` snapshot — the
//! reference plugin's own recorded output, except for
//! `package-directory-packages-glob`, which records ImportLint's deliberately
//! divergent project-root fallback for files outside every `packageDirectory`
//! match. See `tests/conformance/README.md` for the snapshot shape, how these
//! were generated, and the "Documented divergences" section.
//!
//! `tests/conformance/fixtures/{project,packages}` are never modified in place:
//! each test run copies the whole fixture tree into a fresh `TempDir` and builds
//! `<tmp>/project/node_modules/{@fixture-package-third-party,@fixture-package-workspace}/*`
//! there (real copies for third-party packages — mirroring npm's `file:`
//! dependency installation — and relative symlinks for workspace packages —
//! mirroring npm workspaces), since that layout is a `node_modules` install
//! artifact never checked into this repo (see the README's "Fixture package
//! installation" section).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use import_lint::rule::SelfRefOpt;
use import_lint::{Diagnostic, PackageAccessRuleOptions, SelfReferenceMode, check_graph};
use import_lint_cli::runner::RunnerOptions;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

/// One entry in an `expected/<name>.json` snapshot (see README's "Diagnostic JSON
/// shape").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SnapshotDiagnostic {
    file: String,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
    message_id: String,
    message: String,
    identifier: String,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root must exist")
}

fn conformance_dir() -> PathBuf {
    repo_root().join("tests/conformance")
}

/// Recursively copy `src` into `dst` (which must not yet exist, or may be an
/// already-created empty directory). The fixture source tree contains no
/// symlinks (verified — see README), so plain file/dir copying is sufficient.
fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap_or_else(|err| panic!("create_dir_all {}: {err}", dst.display()));
    for entry in fs::read_dir(src).unwrap_or_else(|err| panic!("read_dir {}: {err}", src.display()))
    {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).unwrap_or_else(|err| {
                panic!(
                    "copy {} -> {}: {err}",
                    src_path.display(),
                    dst_path.display()
                )
            });
        } else {
            panic!(
                "unexpected non-file, non-dir entry in fixture tree: {}",
                src_path.display()
            );
        }
    }
}

/// Set up a fresh copy of the fixture tree plus its `node_modules` layout in a new
/// `TempDir`. Returns the temp dir (kept alive by the caller) and the canonicalized
/// `<tmp>/project` path.
fn setup_fixture() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create tempdir");
    let fixtures_src = conformance_dir().join("fixtures");
    copy_dir_recursive(&fixtures_src, tmp.path());

    let project_dir = tmp.path().join("project");
    let packages_dir = tmp.path().join("packages");
    assert!(project_dir.is_dir(), "fixture copy missing project/");
    assert!(packages_dir.is_dir(), "fixture copy missing packages/");

    // Third-party packages: real copies under node_modules/@fixture-package-third-party/*
    // (mirrors npm installing a `file:` devDependency as an actual copy).
    let third_party_src = packages_dir.join("third-party");
    let third_party_dst = project_dir
        .join("node_modules")
        .join("@fixture-package-third-party");
    fs::create_dir_all(&third_party_dst).unwrap();
    for entry in fs::read_dir(&third_party_src).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let name = entry.file_name();
        copy_dir_recursive(&entry.path(), &third_party_dst.join(&name));
    }

    // Workspace packages: relative symlinks under node_modules/@fixture-package-workspace/*
    // (mirrors npm workspaces, which always symlinks rather than copies).
    let workspaces_src = packages_dir.join("workspaces");
    let workspaces_dst = project_dir
        .join("node_modules")
        .join("@fixture-package-workspace");
    fs::create_dir_all(&workspaces_dst).unwrap();
    for entry in fs::read_dir(&workspaces_src).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let name = entry.file_name();
        let link_path = workspaces_dst.join(&name);
        // link lives at <tmp>/project/node_modules/@fixture-package-workspace/<name>;
        // the real directory is at <tmp>/packages/workspaces/<name>. Relative from the
        // link's own location: up out of @fixture-package-workspace/, node_modules/,
        // and project/, then into packages/workspaces/<name>.
        let target = PathBuf::from("../../../packages/workspaces").join(&name);
        std::os::unix::fs::symlink(&target, &link_path).unwrap_or_else(|err| {
            panic!(
                "symlink {} -> {}: {err}",
                link_path.display(),
                target.display()
            )
        });

        // Smoke-check: the symlink resolves to a real directory with the expected
        // package.json name.
        let resolved = fs::canonicalize(&link_path)
            .unwrap_or_else(|err| panic!("resolve symlink {}: {err}", link_path.display()));
        assert!(
            resolved.is_dir(),
            "{} should resolve to a directory",
            link_path.display()
        );
        let pkg_json = resolved.join("package.json");
        assert!(
            pkg_json.is_file(),
            "{} missing package.json",
            resolved.display()
        );
    }

    let project_root = project_dir
        .canonicalize()
        .expect("canonicalize project dir");
    (tmp, project_root)
}

/// Run ImportLint over the fixture project under the named option set (a key into
/// `expected/manifest.json`) and assert its diagnostics match `expected/<name>.json`
/// exactly.
fn run_option_set(name: &str) {
    let manifest_path = conformance_dir().join("expected/manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display())),
    )
    .expect("parse manifest.json");

    let option_set = manifest["optionSets"]
        .get(name)
        .unwrap_or_else(|| panic!("no option set '{name}' in manifest.json"));
    let options: PackageAccessRuleOptions = serde_json::from_value(option_set["options"].clone())
        .unwrap_or_else(|err| panic!("deserialize options for '{name}': {err}"));
    let expected_file = option_set["file"]
        .as_str()
        .unwrap_or_else(|| panic!("option set '{name}' missing 'file'"));

    let expected_path = conformance_dir().join("expected").join(expected_file);
    let expected: Vec<SnapshotDiagnostic> = serde_json::from_str(
        &fs::read_to_string(&expected_path)
            .unwrap_or_else(|err| panic!("read {}: {err}", expected_path.display())),
    )
    .expect("parse expected snapshot");

    let (_tmp, project_root) = setup_fixture();

    let self_reference_mode = match options.treat_self_reference_as {
        SelfRefOpt::Internal => SelfReferenceMode::Internal,
        SelfRefOpt::External => SelfReferenceMode::External,
    };

    let runner_options = RunnerOptions {
        roots: vec![project_root.join("src")],
        project_root: project_root.clone(),
        tsconfig: Some(project_root.join("tsconfig.json")),
        self_reference_mode,
        exclude: Vec::new(),
    };
    let graph = import_lint_cli::run(&runner_options);
    let diagnostics = check_graph(&graph, &options, &project_root);

    let mut source_cache: HashMap<PathBuf, String> = HashMap::new();
    let mut actual: Vec<SnapshotDiagnostic> = diagnostics
        .iter()
        .map(|d| to_snapshot(d, &project_root, &mut source_cache))
        .collect();
    actual.sort_by(|a, b| {
        (&a.file, a.line, a.column, &a.message_id).cmp(&(&b.file, b.line, b.column, &b.message_id))
    });

    let mut expected_sorted = expected.clone();
    expected_sorted.sort_by(|a, b| {
        (&a.file, a.line, a.column, &a.message_id).cmp(&(&b.file, b.line, b.column, &b.message_id))
    });

    if actual != expected_sorted {
        let missing: Vec<&SnapshotDiagnostic> = expected_sorted
            .iter()
            .filter(|e| !actual.contains(e))
            .collect();
        let extra: Vec<&SnapshotDiagnostic> = actual
            .iter()
            .filter(|a| !expected_sorted.contains(a))
            .collect();
        panic!(
            "conformance mismatch for option set '{name}' ({} expected, {} actual)\n\n\
             missing (expected but not produced):\n{}\n\n\
             extra (produced but not expected):\n{}",
            expected_sorted.len(),
            actual.len(),
            serde_json::to_string_pretty(&missing).unwrap(),
            serde_json::to_string_pretty(&extra).unwrap(),
        );
    }
}

fn to_snapshot(
    diagnostic: &Diagnostic,
    project_root: &Path,
    source_cache: &mut HashMap<PathBuf, String>,
) -> SnapshotDiagnostic {
    let source = source_cache
        .entry(diagnostic.path.clone())
        .or_insert_with(|| {
            fs::read_to_string(&diagnostic.path)
                .unwrap_or_else(|err| panic!("read {}: {err}", diagnostic.path.display()))
        });
    let (line, column) = import_lint::diagnostics::line_col(source, diagnostic.span.start);
    let (end_line, end_column) = import_lint::diagnostics::line_col(source, diagnostic.span.end);

    let file = diagnostic
        .path
        .strip_prefix(project_root)
        .unwrap_or(&diagnostic.path)
        .to_string_lossy()
        .replace('\\', "/");

    SnapshotDiagnostic {
        file,
        line,
        column,
        end_line,
        end_column,
        message_id: diagnostic.message_id.as_str().to_string(),
        message: diagnostic.message(),
        identifier: diagnostic.identifier.to_string(),
    }
}

#[test]
fn conformance_default() {
    run_option_set("default");
}
#[test]
fn conformance_index_loophole_false() {
    run_option_set("index-loophole-false");
}

#[test]
fn conformance_index_loophole_false_filename_loophole_true() {
    run_option_set("index-loophole-false-filename-loophole-true");
}

#[test]
fn conformance_default_importability_package() {
    run_option_set("default-importability-package");
}

#[test]
fn conformance_default_importability_package_exclude_source_patterns() {
    run_option_set("default-importability-package-exclude-source-patterns");
}

#[test]
fn conformance_default_importability_private() {
    run_option_set("default-importability-private");
}

#[test]
fn conformance_default_importability_private_self_reference_internal() {
    run_option_set("default-importability-private-self-reference-internal");
}

#[test]
fn conformance_default_importability_private_self_reference_external() {
    run_option_set("default-importability-private-self-reference-external");
}

#[test]
fn conformance_package_directory_no_internal() {
    run_option_set("package-directory-no-internal");
}

#[test]
fn conformance_package_directory_all_star() {
    run_option_set("package-directory-all-star");
}

#[test]
fn conformance_package_directory_no_internal_filename_loophole() {
    run_option_set("package-directory-no-internal-filename-loophole");
}

#[test]
fn conformance_package_directory_packages_glob() {
    run_option_set("package-directory-packages-glob");
}
