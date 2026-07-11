//! Resolver integration tests (PLAN.md §9.3, M2). Each test builds a small fixture
//! tree in a tempdir — no dependency on `tests/conformance`'s `node_modules` being
//! installed.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use import_lint::{ProjectResolver, Provenance, SelfReferenceMode};
use oxc_str::CompactStr;
use tempfile::TempDir;

/// A fixture project tree plus convenience helpers for writing files and building a
/// [`ProjectResolver`] over it.
struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("create tempdir"),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    /// Write `content` to `rel` (relative to the project root), creating parent
    /// directories as needed.
    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("create parent dirs");
        fs::write(&path, content).expect("write fixture file");
        path
    }

    fn resolver_with(
        &self,
        tsconfig: Option<&str>,
        ambient_modules: HashMap<CompactStr, PathBuf>,
        mode: SelfReferenceMode,
    ) -> ProjectResolver {
        ProjectResolver::new(
            self.root(),
            tsconfig.map(|rel| self.path(rel)),
            ambient_modules,
            mode,
        )
    }

    fn resolver(&self, tsconfig: Option<&str>) -> ProjectResolver {
        self.resolver_with(tsconfig, HashMap::new(), SelfReferenceMode::default())
    }
}

// ---- relative imports ----

#[test]
fn relative_import_resolves_internal() {
    let project = Project::new();
    let target = project.write("src/foo.ts", "export const x = 1;\n");
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "./foo"),
        Provenance::Internal(target)
    );
}

#[test]
fn unresolvable_relative_import_is_unresolved() {
    let project = Project::new();
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "./does-not-exist"),
        Provenance::Unresolved
    );
}

// ---- tsconfig paths / baseUrl ----

#[test]
fn tsconfig_paths_alias_resolves_internal() {
    let project = Project::new();
    project.write(
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@app/*": ["src/*"] } } }"#,
    );
    let target = project.write("src/app-target.ts", "export const x = 1;\n");
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(Some("tsconfig.json"));

    assert_eq!(
        resolver.resolve(&importer, "@app/app-target"),
        Provenance::Internal(target)
    );
}

#[test]
fn tsconfig_base_url_bare_specifier_resolves_internal() {
    let project = Project::new();
    project.write(
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": "src" } }"#,
    );
    let target = project.write("src/foo.ts", "export const x = 1;\n");
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(Some("tsconfig.json"));

    assert_eq!(
        resolver.resolve(&importer, "foo"),
        Provenance::Internal(target)
    );
}

// ---- node_modules packages ----

#[test]
fn node_modules_package_with_main_field_is_external() {
    let project = Project::new();
    project.write(
        "node_modules/pkg-main/package.json",
        r#"{ "name": "pkg-main", "main": "index.js" }"#,
    );
    project.write("node_modules/pkg-main/index.js", "module.exports = {};\n");
    project.write(
        "node_modules/pkg-main/index.d.ts",
        "export const x: number;\n",
    );
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "pkg-main"),
        Provenance::External
    );
}

#[test]
fn types_only_package_resolves_external_via_dts_path() {
    let project = Project::new();
    project.write(
        "node_modules/pkg-types-only/package.json",
        r#"{ "name": "pkg-types-only", "types": "index.d.ts" }"#,
    );
    project.write(
        "node_modules/pkg-types-only/index.d.ts",
        "export const x: number;\n",
    );
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "pkg-types-only"),
        Provenance::External
    );
}

#[test]
fn package_with_exports_map_is_external() {
    let project = Project::new();
    project.write(
        "node_modules/pkg-exports/package.json",
        r#"{
            "name": "pkg-exports",
            "exports": { ".": { "types": "./dist/index.d.ts", "default": "./dist/index.js" } }
        }"#,
    );
    project.write("node_modules/pkg-exports/dist/index.js", "export {};\n");
    project.write(
        "node_modules/pkg-exports/dist/index.d.ts",
        "export const x: number;\n",
    );
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "pkg-exports"),
        Provenance::External
    );
}

#[cfg(unix)]
#[test]
fn symlinked_workspace_package_is_external() {
    let project = Project::new();
    project.write(
        "packages/pkg/package.json",
        r#"{ "name": "@ws/pkg", "main": "index.js" }"#,
    );
    project.write("packages/pkg/index.js", "module.exports = {};\n");
    project.write("packages/pkg/index.d.ts", "export const x: number;\n");

    let node_modules_scope = project.path("node_modules/@ws");
    fs::create_dir_all(&node_modules_scope).expect("create node_modules/@ws");
    std::os::unix::fs::symlink(project.path("packages/pkg"), node_modules_scope.join("pkg"))
        .expect("create workspace symlink");

    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(resolver.resolve(&importer, "@ws/pkg"), Provenance::External);
}

// ---- node builtins ----

#[test]
fn node_builtin_bare_is_external() {
    let project = Project::new();
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(resolver.resolve(&importer, "path"), Provenance::External);
}

#[test]
fn node_builtin_with_node_prefix_is_external() {
    let project = Project::new();
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "node:path"),
        Provenance::External
    );
}

#[test]
fn node_prefixed_nonexistent_builtin_is_unresolved() {
    let project = Project::new();
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver(None);

    assert_eq!(
        resolver.resolve(&importer, "node:nonexistent-thing"),
        Provenance::Unresolved
    );
}

// ---- ambient modules ----

#[test]
fn ambient_module_beats_resolver_for_bare_specifier() {
    let project = Project::new();
    let decl_file = project.write(
        "src/types/ambient.d.ts",
        r#"declare module "ambient-thing" { export const x: number; }"#,
    );
    // Also install a real node_modules package under the same name, to prove the
    // ambient map wins even when the resolver *could* otherwise resolve it.
    project.write(
        "node_modules/ambient-thing/package.json",
        r#"{ "name": "ambient-thing", "main": "index.js" }"#,
    );
    project.write(
        "node_modules/ambient-thing/index.js",
        "module.exports = {};\n",
    );

    let importer = project.write("src/importer.ts", "");
    let mut ambient = HashMap::new();
    ambient.insert(CompactStr::from("ambient-thing"), decl_file.clone());
    let resolver = project.resolver_with(None, ambient, SelfReferenceMode::default());

    assert_eq!(
        resolver.resolve(&importer, "ambient-thing"),
        Provenance::Internal(decl_file)
    );
}

// ---- self-reference (spec §4.6) ----

#[test]
fn self_reference_is_external_in_external_mode() {
    let project = Project::new();
    project.write(
        "package.json",
        r#"{
            "name": "mypkg",
            "exports": { "./sub": { "types": "./src/sub.d.ts", "default": "./src/sub.js" } }
        }"#,
    );
    project.write("src/sub.d.ts", "export const x: number;\n");
    project.write("src/sub.js", "export {};\n");
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver_with(None, HashMap::new(), SelfReferenceMode::External);

    assert_eq!(
        resolver.resolve(&importer, "mypkg/sub"),
        Provenance::External
    );
}

#[test]
fn self_reference_resolves_internal_in_internal_mode() {
    let project = Project::new();
    project.write(
        "package.json",
        r#"{
            "name": "mypkg",
            "exports": { "./sub": { "types": "./src/sub.d.ts", "default": "./src/sub.js" } }
        }"#,
    );
    let target = project.write("src/sub.d.ts", "export const x: number;\n");
    project.write("src/sub.js", "export {};\n");
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver_with(None, HashMap::new(), SelfReferenceMode::Internal);

    assert_eq!(
        resolver.resolve(&importer, "mypkg/sub"),
        Provenance::Internal(target)
    );
}

/// A self-referenced subpath whose `exports` target names the compiled `.js` file
/// must resolve to the `.ts` source actually on disk (TS-style extension
/// substitution via `extension_alias`) — this is exactly the conformance
/// fixture's `"./self-reference": "./src/self-reference/index.js"` shape.
#[test]
fn self_reference_exports_map_js_target_resolves_ts_source() {
    let project = Project::new();
    project.write(
        "package.json",
        r#"{
            "name": "mypkg",
            "exports": { "./sub": "./src/sub.js" }
        }"#,
    );
    let target = project.write("src/sub.ts", "export const x = 1;\n");
    let importer = project.write("src/importer.ts", "");
    let resolver = project.resolver_with(None, HashMap::new(), SelfReferenceMode::Internal);

    assert_eq!(
        resolver.resolve(&importer, "mypkg/sub"),
        Provenance::Internal(target)
    );
}
