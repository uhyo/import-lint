//! Integration tests for discovery (`import_lint_cli::walk`) and the discovery+link
//! pipeline (`import_lint_cli::runner::run`), driving the library directly (PLAN.md
//! M2) rather than spawning the binary — these need programmatic access to the
//! resulting `Vec<PathBuf>` / `ModuleGraph`, not just CLI output.

use std::fs;
use std::path::{Path, PathBuf};

use import_lint::{Provenance, SelfReferenceMode};
use import_lint_cli::runner::RunnerOptions;
use import_lint_cli::{run, walk};
use tempfile::TempDir;

fn write(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|err| panic!("canonicalize {}: {err}", path.display()))
}

#[test]
fn discovery_finds_nested_files_and_skips_ignored_and_node_modules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "ignored.ts\n");
    write(root, "src/a.ts", "export const a = 1;\n");
    write(root, "src/nested/b.ts", "export const b = 1;\n");
    write(root, "src/ignored.ts", "export const ignored = 1;\n");
    write(root, "src/data.json", "{}\n");
    write(root, "node_modules/pkg/index.ts", "export const pkg = 1;\n");

    let found = walk(&[root.to_path_buf()]);

    assert!(found.contains(&canonical(&root.join("src/a.ts"))));
    assert!(found.contains(&canonical(&root.join("src/nested/b.ts"))));
    assert!(!found.iter().any(|p| p.ends_with("ignored.ts")));
    assert!(!found.iter().any(|p| p.ends_with("data.json")));
    assert!(
        !found
            .iter()
            .any(|p| p.components().any(|c| c.as_os_str() == "node_modules"))
    );

    // Deterministic: sorted, no duplicates.
    let mut sorted = found.clone();
    sorted.sort();
    assert_eq!(found, sorted);
}

#[test]
fn discovery_skips_nonexistent_root_without_panicking() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("does-not-exist");

    let found = walk(&[missing]);

    assert!(found.is_empty());
}

fn default_options(root: &Path, project_root: &Path) -> RunnerOptions {
    RunnerOptions {
        roots: vec![root.to_path_buf()],
        project_root: project_root.to_path_buf(),
        tsconfig: RunnerOptions::default_tsconfig(project_root),
        self_reference_mode: SelfReferenceMode::default(),
    }
}

#[test]
fn pipeline_resolves_relative_imports_and_records_lint_targets() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(
        root,
        "src/a.ts",
        "import { b } from \"./b\";\nexport { b };\n",
    );
    write(root, "src/b.ts", "export const b = 1;\n");

    let options = default_options(&root.join("src"), root);
    let graph = run(&options);

    let a = canonical(&root.join("src/a.ts"));
    let b = canonical(&root.join("src/b.ts"));

    assert!(graph.lint_targets.contains(&a));
    assert!(graph.lint_targets.contains(&b));
    assert_eq!(graph.resolution(&a, "./b"), Some(&Provenance::Internal(b)));
}

#[test]
fn fixpoint_extracts_internal_targets_outside_the_walked_root_but_not_as_lint_targets() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(
        root,
        "src/a.ts",
        "import { bVal } from \"../outside/b\";\nconsole.log(bVal);\n",
    );
    write(
        root,
        "outside/b.ts",
        "import { cVal } from \"./c\";\nexport const bVal = cVal;\n",
    );
    write(root, "outside/c.ts", "export const cVal = 1;\n");

    let options = default_options(&root.join("src"), root);
    let graph = run(&options);

    let a = canonical(&root.join("src/a.ts"));
    let b = canonical(&root.join("outside/b.ts"));
    let c = canonical(&root.join("outside/c.ts"));

    // Both b and c were reached only through the fixpoint loop (b via a's import,
    // c via b's import) and must be present as export-table lookup targets...
    assert!(graph.files.contains_key(&b));
    assert!(graph.files.contains_key(&c));
    // ...but neither was walked, so neither is a lint target.
    assert!(graph.lint_targets.contains(&a));
    assert!(!graph.lint_targets.contains(&b));
    assert!(!graph.lint_targets.contains(&c));

    assert_eq!(
        graph.resolution(&a, "../outside/b"),
        Some(&Provenance::Internal(b.clone()))
    );
    assert_eq!(graph.resolution(&b, "./c"), Some(&Provenance::Internal(c)));
}

#[test]
fn ambient_module_declaration_resolves_bare_specifier_to_declaring_dts_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(
        root,
        "src/types.d.ts",
        "declare module \"virtual-mod\" {\n  export const x: number;\n}\n",
    );
    write(
        root,
        "src/user.ts",
        "import { x } from \"virtual-mod\";\nconsole.log(x);\n",
    );

    let options = default_options(&root.join("src"), root);
    let graph = run(&options);

    let user = canonical(&root.join("src/user.ts"));
    let types = canonical(&root.join("src/types.d.ts"));

    assert_eq!(
        graph.resolution(&user, "virtual-mod"),
        Some(&Provenance::Internal(types))
    );
}

#[test]
fn node_modules_package_is_external_and_missing_specifier_is_unresolved() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(
        root,
        "node_modules/some-pkg/package.json",
        "{\"name\": \"some-pkg\", \"main\": \"index.js\"}\n",
    );
    write(
        root,
        "node_modules/some-pkg/index.js",
        "module.exports = {};\n",
    );
    write(
        root,
        "src/user.ts",
        "import { x } from \"some-pkg\";\nimport { y } from \"missing-pkg\";\nconsole.log(x, y);\n",
    );

    let options = default_options(&root.join("src"), root);
    let graph = run(&options);

    let user = canonical(&root.join("src/user.ts"));

    assert_eq!(
        graph.resolution(&user, "some-pkg"),
        Some(&Provenance::External)
    );
    assert_eq!(
        graph.resolution(&user, "missing-pkg"),
        Some(&Provenance::Unresolved)
    );
}

#[test]
fn parse_error_file_is_skipped_without_aborting_the_run() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "src/good.ts", "export const x = 1;\n");
    // Unterminated string literal -> guaranteed parse error.
    write(root, "src/broken.ts", "export const y = \"unterminated;\n");

    let options = default_options(&root.join("src"), root);
    let graph = run(&options);

    let good = canonical(&root.join("src/good.ts"));
    let broken = canonical(&root.join("src/broken.ts"));

    assert!(graph.files.contains_key(&good));
    assert!(!graph.files.contains_key(&broken));
    // Still recorded as a lint target (it was walked); it simply has no
    // `FileModuleInfo` to check against, so M3 finds nothing to lint there.
    assert!(graph.lint_targets.contains(&broken));
}

#[test]
fn runs_over_the_conformance_fixture_tree_without_panicking() {
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/fixtures/project");
    let fixture_src = fixture_root.join("src");
    assert!(
        fixture_src.is_dir(),
        "expected fixture src dir at {}",
        fixture_src.display()
    );

    let options = default_options(&fixture_src, &fixture_root);
    let graph = run(&options);

    assert!(!graph.files.is_empty());

    // At least one relative import in the fixture tree must resolve internally
    // (this doesn't depend on the fixture's node_modules, which isn't installed).
    let has_internal_relative_resolution = graph.resolutions.iter().any(|((_, specifier), p)| {
        specifier.starts_with('.') && matches!(p, Provenance::Internal(_))
    });
    assert!(has_internal_relative_resolution);
}
