//! Extraction tests, one per declaration form from
//! docs/research/eslint-plugin-import-access-spec.md §3.2, plus the JSDoc->Access
//! tag-scanning rules from §3.1.

use std::path::Path;

use import_lint::{Access, EntryKind, FileModuleInfo, extract_file};
use oxc_allocator::Allocator;
use oxc_span::SourceType;

fn extract_ts(source: &str) -> FileModuleInfo {
    extract_with(source, SourceType::ts())
}

fn extract_dts(source: &str) -> FileModuleInfo {
    extract_with(source, SourceType::d_ts())
}

fn extract_with(source: &str, source_type: SourceType) -> FileModuleInfo {
    let allocator = Allocator::default();
    extract_file(Path::new("test.ts"), source, source_type, &allocator)
}

fn access_of(info: &FileModuleInfo, name: &str) -> Option<Access> {
    info.export_table
        .get(name)
        .unwrap_or_else(|| panic!("no export_table entry for {name}"))
        .access
}

// ---- JSDoc -> Access (spec §3.1) ----

#[test]
fn no_jsdoc_means_no_access() {
    let info = extract_ts("export const x = 1;");
    assert_eq!(access_of(&info, "x"), None);
}

#[test]
fn jsdoc_package_tag() {
    let info = extract_ts("/** @package */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), Some(Access::Package));
}

#[test]
fn jsdoc_private_tag() {
    let info = extract_ts("/** @private */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), Some(Access::Private));
}

#[test]
fn jsdoc_public_tag() {
    let info = extract_ts("/** @public */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), Some(Access::Public));
}

#[test]
fn jsdoc_access_tag_variants() {
    for (level, expected) in [
        ("public", Access::Public),
        ("package", Access::Package),
        ("private", Access::Private),
    ] {
        let source = format!("/**\n * @access {level}\n */\nexport const x = 1;");
        let info = extract_ts(&source);
        assert_eq!(access_of(&info, "x"), Some(expected), "for @access {level}");
    }
}

#[test]
fn jsdoc_unrecognized_tag_falls_through_to_none() {
    let info = extract_ts("/** @deprecated use y instead */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), None);
}

#[test]
fn jsdoc_tag_order_within_one_block_first_match_wins() {
    let info = extract_ts("/**\n * @private\n * @package\n */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), Some(Access::Private));
}

#[test]
fn jsdoc_stacked_blocks_scan_farthest_to_nearest() {
    // Farthest block has no recognized tag, nearest one does -> nearest wins.
    let info = extract_ts("/** just a comment */\n/** @package */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), Some(Access::Package));

    // Farthest block already has a recognized tag -> it wins over the nearer one.
    let info = extract_ts("/** @private */\n/** @package */\nexport const x = 1;");
    assert_eq!(access_of(&info, "x"), Some(Access::Private));
}

// ---- export forms (spec §3.2) ----

#[test]
fn export_const_let_var_and_destructuring() {
    let info = extract_ts(
        "/** @package */\nexport const { a, b: [c, ...rest], ...others } = obj as any;\nexport let y = 1;\nexport var z = 2;",
    );
    for name in ["a", "c", "rest", "others"] {
        assert_eq!(access_of(&info, name), Some(Access::Package), "for {name}");
    }
    assert_eq!(access_of(&info, "y"), None);
    assert_eq!(access_of(&info, "z"), None);
}

#[test]
fn export_function_and_class() {
    let info = extract_ts(
        "/** @package */\nexport function foo() {}\n/** @private */\nexport class Bar {}",
    );
    assert_eq!(access_of(&info, "foo"), Some(Access::Package));
    assert_eq!(access_of(&info, "Bar"), Some(Access::Private));
}

#[test]
fn export_interface_and_type_alias() {
    let info = extract_ts(
        "/** @package */\nexport interface Foo {}\n/** @private */\nexport type Bar = string;",
    );
    assert_eq!(access_of(&info, "Foo"), Some(Access::Package));
    assert_eq!(access_of(&info, "Bar"), Some(Access::Private));
}

#[test]
fn export_enum_and_namespace_are_not_qualifying_exports() {
    // The reference's `findExportedDeclaration` walk-up only recognizes
    // Function/Class/VariableStatement/TypeAlias/Interface/ExportDeclaration/
    // ExportAssignment (src/utils/findExportableDeclaration.ts) — `export enum`/
    // `export namespace` are never checked there, so we deliberately create no
    // export_table entry (a lookup miss is later treated as "always allowed").
    let info = extract_ts(
        "/** @private */\nexport enum E { A }\n/** @private */\nexport namespace N { export const x = 1; }",
    );
    assert!(!info.export_table.contains_key("E"));
    assert!(!info.export_table.contains_key("N"));
    assert!(!info.export_table.contains_key("x"));
}

#[test]
fn export_default_expression_function_and_class() {
    let info = extract_ts("/** @package */\nexport default 42;");
    assert_eq!(access_of(&info, "default"), Some(Access::Package));

    let info = extract_ts("export default function foo() {}");
    assert_eq!(access_of(&info, "default"), None);

    let info = extract_ts("/** @private */\nexport default class {}");
    assert_eq!(access_of(&info, "default"), Some(Access::Private));
}

#[test]
fn export_assignment_export_equals() {
    let info = extract_ts("const v = 1;\n/**\n * @package\n */\nexport = v;");
    assert_eq!(access_of(&info, "export="), Some(Access::Package));
    // "default" must NOT be used as the key for `export =` (spike S4).
    assert!(!info.export_table.contains_key("default"));
}

#[test]
fn export_assignment_ignores_jsdoc_on_the_referenced_declaration() {
    // Spike S4 case 2: JSDoc on the `const` a few lines above the `export =`
    // statement must NOT be picked up — only JSDoc directly on `export =` counts.
    let info = extract_ts("/**\n * @package\n */\nconst v = 1;\nexport = v;");
    assert_eq!(access_of(&info, "export="), None);
}

#[test]
fn local_named_export_no_source() {
    let info = extract_ts("const x = 1;\n/** @package */\nexport { x };");
    assert_eq!(access_of(&info, "x"), Some(Access::Package));
    assert!(
        info.checked_entries.is_empty(),
        "a local `export {{x}}` is not itself a checked entry"
    );
}

#[test]
fn local_named_export_with_alias() {
    let info = extract_ts("const x = 1;\n/** @private */\nexport { x as y };");
    assert_eq!(access_of(&info, "y"), Some(Access::Private));
    assert!(!info.export_table.contains_key("x"));
}

#[test]
fn reexport_named_from_source() {
    let source = r#"export { z } from "./m";"#;
    let info = extract_ts(source);
    assert_eq!(info.checked_entries.len(), 1);
    let entry = &info.checked_entries[0];
    assert_eq!(entry.kind, EntryKind::ReExport);
    assert_eq!(entry.imported_name.as_str(), "z");
    assert_eq!(entry.specifier.as_str(), "./m");
    // Exact span of the whole specifier (`z`), per D12.
    let expected_start = source_index(source, "z");
    assert_eq!(entry.span.start, expected_start);
    assert_eq!(access_of(&info, "z"), None);
    assert!(info.specifiers.iter().any(|s| s.as_str() == "./m"));
}

#[test]
fn reexport_named_with_alias_from_source() {
    let source = r#"export { x as y } from "./m";"#;
    let info = extract_ts(source);
    assert_eq!(info.checked_entries.len(), 1);
    let entry = &info.checked_entries[0];
    assert_eq!(entry.kind, EntryKind::ReExport);
    // imported_name is the name at the source (`x`), not the exported alias (`y`).
    assert_eq!(entry.imported_name.as_str(), "x");
    // Span covers the whole `x as y` specifier (D12).
    let spec_start = source_index(source, "x as y");
    let spec_end = spec_start + u32::try_from("x as y".len()).unwrap();
    assert_eq!(entry.span.start, spec_start);
    assert_eq!(entry.span.end, spec_end);
    // export_table is keyed by the exported name `y`, not `x`.
    assert!(info.export_table.contains_key("y"));
    assert!(!info.export_table.contains_key("x"));
}

#[test]
fn star_export_bare_is_not_checked_and_not_in_export_table() {
    let info = extract_ts(r#"export * from "./barrel";"#);
    assert_eq!(info.star_exports, vec!["./barrel"]);
    assert!(info.checked_entries.is_empty());
    assert!(info.export_table.is_empty());
}

#[test]
fn star_export_as_namespace_is_an_ordinary_named_export() {
    // Spike S1 Q5: `export * as ns from "./m"` creates a checkable export table
    // entry `ns` of *this* file — not a star export, and not checked as an import.
    let info = extract_ts("/** @private */\nexport * as ns from \"./inner\";");
    assert_eq!(access_of(&info, "ns"), Some(Access::Private));
    assert!(info.star_exports.is_empty());
    assert!(info.checked_entries.is_empty());
}

#[test]
fn import_named_and_aliased() {
    let source = "import { x } from \"./m\";\nimport { x as y } from \"./m2\";";
    let info = extract_ts(source);
    assert_eq!(info.checked_entries.len(), 2);

    let first = &info.checked_entries[0];
    assert_eq!(first.kind, EntryKind::Import);
    assert_eq!(first.imported_name.as_str(), "x");
    assert_eq!(first.specifier.as_str(), "./m");

    let second = &info.checked_entries[1];
    assert_eq!(second.kind, EntryKind::Import);
    // reported name is the *original* exported name, not the local alias.
    assert_eq!(second.imported_name.as_str(), "x");
    assert_eq!(second.specifier.as_str(), "./m2");
    let spec_start = source_index(source, "x as y");
    let spec_end = spec_start + u32::try_from("x as y".len()).unwrap();
    assert_eq!(second.span.start, spec_start);
    assert_eq!(second.span.end, spec_end);
}

#[test]
fn import_default() {
    let info = extract_ts("import D from \"./m\";");
    assert_eq!(info.checked_entries.len(), 1);
    let entry = &info.checked_entries[0];
    assert_eq!(entry.kind, EntryKind::ImportDefault);
    assert_eq!(entry.imported_name.as_str(), "default");
    assert_eq!(entry.specifier.as_str(), "./m");
}

#[test]
fn import_default_as_named_specifier() {
    // `import { default as x } from "./m"` is syntactically an ImportSpecifier
    // (not an ImportDefaultSpecifier) named "default" (spec §3.2).
    let info = extract_ts("import { default as x } from \"./m\";");
    assert_eq!(info.checked_entries.len(), 1);
    let entry = &info.checked_entries[0];
    assert_eq!(entry.kind, EntryKind::Import);
    assert_eq!(entry.imported_name.as_str(), "default");
}

#[test]
fn import_namespace_is_not_checked() {
    let info = extract_ts("import * as ns from \"./m\";");
    assert!(info.checked_entries.is_empty());
    assert!(info.specifiers.iter().any(|s| s.as_str() == "./m"));
}

#[test]
fn import_side_effect_only_is_not_checked() {
    let info = extract_ts("import \"./m\";");
    assert!(info.checked_entries.is_empty());
    assert!(info.specifiers.iter().any(|s| s.as_str() == "./m"));
}

#[test]
fn import_type_only_is_checked_same_as_value_import() {
    // Spec §3.2: "same code path, no special-casing of importKind" — not covered by
    // any reference test, but this is what the spec's source-level analysis found.
    let info = extract_ts("import type { X } from \"./m\";");
    assert_eq!(info.checked_entries.len(), 1);
    assert_eq!(info.checked_entries[0].imported_name.as_str(), "X");

    let info = extract_ts("import { type Y } from \"./m\";");
    assert_eq!(info.checked_entries.len(), 1);
    assert_eq!(info.checked_entries[0].imported_name.as_str(), "Y");
}

#[test]
fn export_type_reexport_is_checked_same_as_value_reexport() {
    let info = extract_ts(r#"export type { X } from "./m";"#);
    assert_eq!(info.checked_entries.len(), 1);
    assert_eq!(info.checked_entries[0].kind, EntryKind::ReExport);
    assert!(info.export_table.contains_key("X"));
}

#[test]
fn dynamic_import_and_import_equals_are_not_checked() {
    let info = extract_ts(
        "const p = import(\"./m\");\nimport eq = require(\"./m2\");\nconst r = require(\"./m3\");",
    );
    assert!(info.checked_entries.is_empty());
}

#[test]
fn specifiers_are_deduplicated() {
    let info = extract_ts("import { a } from \"./m\";\nimport { b } from \"./m\";");
    assert_eq!(
        info.specifiers
            .iter()
            .filter(|s| s.as_str() == "./m")
            .count(),
        1
    );
}

// ---- ambient modules (D6) ----

#[test]
fn ambient_module_string_literal_is_recorded_and_its_exports_are_collected() {
    let info = extract_dts(
        "declare module \"generated-package\" {\n  /** @package */\n  export const someValue: string;\n}",
    );
    assert_eq!(info.ambient_modules, vec!["generated-package"]);
    assert_eq!(access_of(&info, "someValue"), Some(Access::Package));
}

#[test]
fn ts_namespace_identifier_is_not_an_ambient_module() {
    let info = extract_dts("declare namespace Foo {\n  export const x = 1;\n}");
    assert!(info.ambient_modules.is_empty());
    // Namespace members are not reachable via ordinary import bindings and must
    // not leak into the file's flat export table.
    assert!(!info.export_table.contains_key("x"));
}

fn source_index(source: &str, needle: &str) -> u32 {
    u32::try_from(source.find(needle).expect("needle not found in source")).unwrap()
}
