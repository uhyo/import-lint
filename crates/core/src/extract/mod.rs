//! Parse one file into an owned [`FileModuleInfo`] (PLAN-v1.md §2.2, §3, M1).
//!
//! This is the only module that touches oxc AST/arena lifetimes: [`extract`] parses,
//! runs semantic analysis (needed for JSDoc lookup), walks the resulting `Program`
//! directly (rather than going through `ModuleRecord`, which does not carry the
//! precise "whole specifier including `as alias`" spans this port needs — see D12),
//! and copies out only owned data before the allocator is reset/dropped by the caller.

mod jsdoc;
pub mod module_info;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportAllDeclaration, ExportDefaultDeclaration,
    ExportNamedDeclaration, ImportDeclaration, ImportDeclarationSpecifier, ModuleExportName,
    Statement, TSExportAssignment, TSModuleDeclaration, TSModuleDeclarationBody,
    TSModuleDeclarationName,
};
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder};
use oxc_span::{SourceType, Span};
use oxc_str::CompactStr;

pub use module_info::{Access, CheckedEntry, EntryKind, ExportInfo, FileModuleInfo};

/// Parse `source_text` (already read from `path`) and extract its owned module
/// summary. `allocator` backs the parse; the caller is expected to reset/drop it
/// after this call returns (this function never lets arena data escape).
pub fn extract<'a>(
    path: &Path,
    source_text: &'a str,
    source_type: SourceType,
    allocator: &'a Allocator,
) -> FileModuleInfo {
    let ret = Parser::new(allocator, source_text, source_type).parse();

    // `new_linter()` is the documented-safe way to build `Semantic` here (enables the
    // full node store + syntax checking); JSDoc attachment itself runs unconditionally
    // once the `jsdoc` feature is on, but there's no reason to opt out of the safer
    // default (see docs/research/spike-s2-jsdoc-attachment.md's "headline finding").
    let semantic = SemanticBuilder::new_linter().build(&ret.program).semantic;

    let mut extractor = Extractor::new(path, semantic, source_text);
    extractor.visit_statements(&ret.program.body);
    extractor.out
}

struct Extractor<'a> {
    semantic: Semantic<'a>,
    source_text: &'a str,
    out: FileModuleInfo,
}

impl<'a> Extractor<'a> {
    fn new(path: &Path, semantic: Semantic<'a>, source_text: &'a str) -> Self {
        Self {
            semantic,
            source_text,
            out: FileModuleInfo {
                path: path.to_path_buf(),
                checked_entries: Vec::new(),
                export_table: HashMap::new(),
                star_exports: Vec::new(),
                ambient_modules: Vec::new(),
                specifiers: Vec::new(),
            },
        }
    }

    fn access_for_span(&self, span: Span) -> Option<Access> {
        jsdoc::access_for_span(&self.semantic, self.source_text, span)
    }

    /// Record a module specifier for the link phase, if not already present.
    fn add_specifier(&mut self, specifier: CompactStr) {
        if !self.out.specifiers.contains(&specifier) {
            self.out.specifiers.push(specifier);
        }
    }

    /// Visit a list of top-level module statements. Called both for the file's own
    /// `Program.body` and (recursively) for the body of a `declare module "x" { ... }`
    /// block, since resolution maps a bare specifier straight to the declaring file
    /// (D6) and `lookup()` needs the ambient module's exports in *this* file's flat
    /// `export_table` regardless of the nesting they were written at.
    fn visit_statements(&mut self, statements: &[Statement<'a>]) {
        for stmt in statements {
            match stmt {
                Statement::ImportDeclaration(decl) => self.import_declaration(decl),
                Statement::ExportNamedDeclaration(decl) => self.export_named_declaration(decl),
                Statement::ExportDefaultDeclaration(decl) => {
                    self.export_default_declaration(decl);
                }
                Statement::ExportAllDeclaration(decl) => self.export_all_declaration(decl),
                Statement::TSExportAssignment(decl) => self.export_assignment(decl),
                Statement::TSModuleDeclaration(decl) => self.ts_module_declaration(decl),
                _ => {}
            }
        }
    }

    /// `import ... from "m"`: named specifiers, the default specifier, and re-export
    /// specifiers are all "checked entries" (D1); `import * as ns` is not (namespace
    /// member access is never checked by the reference plugin).
    fn import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        let specifier = decl.source.value.to_compact_str();
        self.add_specifier(specifier.clone());

        let Some(specifiers) = &decl.specifiers else {
            // Side-effect-only `import "m";` — nothing to check.
            return;
        };
        for spec in specifiers {
            match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    let (imported_name, _) = module_export_name(&s.imported);
                    self.out.checked_entries.push(CheckedEntry {
                        kind: EntryKind::Import,
                        imported_name,
                        specifier: specifier.clone(),
                        span: s.span,
                    });
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    self.out.checked_entries.push(CheckedEntry {
                        kind: EntryKind::ImportDefault,
                        imported_name: CompactStr::from("default"),
                        specifier: specifier.clone(),
                        span: s.span,
                    });
                }
                // `import * as ns from "m"` (D1: never checked).
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {}
            }
        }
    }

    /// Covers `export const/function/class/interface/type ...`, local `export { x };`,
    /// and `export { x as y } from "m";`. All specifiers/declarators in one statement
    /// share the same JSDoc lookup, since oxc attaches JSDoc to the whole
    /// `ExportNamedDeclaration` wrapper, never an inner node (spike S2).
    fn export_named_declaration(&mut self, decl: &ExportNamedDeclaration<'a>) {
        let access = self.access_for_span(decl.span);

        if let Some(declaration) = &decl.declaration {
            self.export_declaration(declaration, access);
        }

        for spec in &decl.specifiers {
            let (exported_name, _) = module_export_name(&spec.exported);
            if let Some(source) = &decl.source {
                // `export { x as y } from "m";` — a re-export: also a checked entry,
                // using the name at the source module (`x`, i.e. `spec.local`), not
                // the exported alias.
                let specifier = source.value.to_compact_str();
                self.add_specifier(specifier.clone());
                let (imported_name, _) = module_export_name(&spec.local);
                self.out.checked_entries.push(CheckedEntry {
                    kind: EntryKind::ReExport,
                    imported_name,
                    specifier,
                    span: spec.span,
                });
            }
            // Local `export { x };` (no `from`) is not itself a checked entry — only
            // `x`'s own import (if any) is checked; but it does introduce a qualifying
            // export-table entry either way, per spec §3.1 ("ExportDeclaration ...
            // returned unconditionally").
            insert_export(&mut self.out.export_table, exported_name, access, spec.span);
        }
    }

    /// One `Declaration` embedded in `export <declaration>;`. Only the forms the
    /// reference's `findExportedDeclaration` walk-up recognizes as qualifying exports
    /// get an entry — `FunctionDeclaration`/`ClassDeclaration`/`VariableStatement`/
    /// `TypeAliasDeclaration`/`InterfaceDeclaration` (spec §3.1). Notably, `enum` and
    /// `namespace`/`module` declarations are NOT in that list in the reference
    /// implementation (`src/utils/findExportableDeclaration.ts` has no
    /// `isEnumDeclaration`/`isModuleDeclaration` check), so `export enum E {}` and
    /// `export namespace N {}` are never checked there — we deliberately create no
    /// export_table entry for them either, so a `lookup()` miss on those names
    /// correctly falls through to "not found" (which downstream milestones must
    /// treat as "allow", matching the reference's silent skip). See M1 final report.
    fn export_declaration(&mut self, declaration: &Declaration<'a>, access: Option<Access>) {
        match declaration {
            Declaration::VariableDeclaration(var_decl) => {
                for declarator in &var_decl.declarations {
                    let mut names = Vec::new();
                    collect_binding_names(&declarator.id, &mut names);
                    for (name, span) in names {
                        insert_export(&mut self.out.export_table, name, access, span);
                    }
                }
            }
            Declaration::FunctionDeclaration(func) => {
                if let Some(id) = &func.id {
                    insert_export(
                        &mut self.out.export_table,
                        CompactStr::from(id.name.as_str()),
                        access,
                        id.span,
                    );
                }
            }
            Declaration::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    insert_export(
                        &mut self.out.export_table,
                        CompactStr::from(id.name.as_str()),
                        access,
                        id.span,
                    );
                }
            }
            Declaration::TSTypeAliasDeclaration(ta) => {
                insert_export(
                    &mut self.out.export_table,
                    CompactStr::from(ta.id.name.as_str()),
                    access,
                    ta.id.span,
                );
            }
            Declaration::TSInterfaceDeclaration(iface) => {
                insert_export(
                    &mut self.out.export_table,
                    CompactStr::from(iface.id.name.as_str()),
                    access,
                    iface.id.span,
                );
            }
            Declaration::TSEnumDeclaration(_)
            | Declaration::TSModuleDeclaration(_)
            | Declaration::TSGlobalDeclaration(_)
            | Declaration::TSImportEqualsDeclaration(_) => {}
        }
    }

    /// `export default ...;` is always a qualifying export (spec §3.1: `ExportAssignment`
    /// forms are "returned unconditionally"), regardless of what it's exporting.
    fn export_default_declaration(&mut self, decl: &ExportDefaultDeclaration<'a>) {
        let access = self.access_for_span(decl.span);
        insert_export(
            &mut self.out.export_table,
            CompactStr::from("default"),
            access,
            decl.span,
        );
    }

    /// `export * from "m";` (recorded in `star_exports`, never checked at this
    /// statement — the reference has no `ExportAllDeclaration` visitor, D1) and
    /// `export * as ns from "m";` (an ordinary named export `ns` of *this* file,
    /// confirmed by spike S1 Q5 — a real one-hop stop, unlike bare `export *`).
    fn export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        let specifier = decl.source.value.to_compact_str();
        self.add_specifier(specifier.clone());
        match &decl.exported {
            Some(exported) => {
                let (name, _) = module_export_name(exported);
                let access = self.access_for_span(decl.span);
                insert_export(&mut self.out.export_table, name, access, decl.span);
            }
            None => {
                self.out.star_exports.push(specifier);
            }
        }
    }

    /// `export = expr;` (spike S4): the export-table key is the literal `"export="`
    /// (TypeScript's `InternalSymbolName.ExportEquals`), distinct from `"default"`.
    /// Only JSDoc directly on this statement counts — never the JSDoc on whatever
    /// declaration `expr` happens to reference.
    fn export_assignment(&mut self, decl: &TSExportAssignment<'a>) {
        let access = self.access_for_span(decl.span);
        insert_export(
            &mut self.out.export_table,
            CompactStr::from("export="),
            access,
            decl.span,
        );
    }

    /// `declare module "x" { ... }`. Ambient module names are collected
    /// unconditionally regardless of file extension (D6 — the `.d.ts`-only filter is
    /// the link phase's job); a `namespace Foo {}` / `module Foo {}` (identifier name,
    /// not a string literal) is a plain TS namespace, not an ambient module — its
    /// members are reachable only via namespace member access, which is out of scope
    /// (D1), so we neither record it nor descend into its body.
    fn ts_module_declaration(&mut self, decl: &TSModuleDeclaration<'a>) {
        let TSModuleDeclarationName::StringLiteral(lit) = &decl.id else {
            return;
        };
        self.out.ambient_modules.push(lit.value.to_compact_str());
        match &decl.body {
            Some(TSModuleDeclarationBody::TSModuleBlock(block)) => {
                self.visit_statements(&block.body);
            }
            Some(TSModuleDeclarationBody::TSModuleDeclaration(nested)) => {
                self.ts_module_declaration(nested);
            }
            None => {}
        }
    }
}

/// Insert an export-table entry, handling the (rare) case of the same name being
/// exported more than once in a file — e.g. TS interface declaration merging, or two
/// export statements naming the same local binding. The first occurrence's span wins
/// (closest to the reference's "first declaration" walk-up target); a later
/// occurrence may still supply an access level if none was found yet, approximating
/// the reference's behavior of aggregating JSDoc tags across all merged declarations
/// (spec §3.2 "Declaration merging" row) without needing a real merged-symbol model.
fn insert_export(
    table: &mut HashMap<CompactStr, ExportInfo>,
    name: CompactStr,
    access: Option<Access>,
    span: Span,
) {
    match table.entry(name) {
        Entry::Vacant(v) => {
            v.insert(ExportInfo { access, span });
        }
        Entry::Occupied(mut o) => {
            if o.get().access.is_none()
                && let Some(access) = access
            {
                o.get_mut().access = Some(access);
            }
        }
    }
}

/// Extract the bound name(s) and span(s) from a binding pattern, recursing through
/// destructuring (`export const { a, b: [c] } = obj;` exports `a` and `c`).
fn collect_binding_names(pattern: &BindingPattern, out: &mut Vec<(CompactStr, Span)>) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            out.push((CompactStr::from(id.name.as_str()), id.span));
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                collect_binding_names(&prop.value, out);
            }
            if let Some(rest) = &obj.rest {
                collect_binding_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                collect_binding_names(elem, out);
            }
            if let Some(rest) = &arr.rest {
                collect_binding_names(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            collect_binding_names(&assign.left, out);
        }
    }
}

/// The name + span carried by a `ModuleExportName` (the `local`/`exported`/`imported`
/// slot of an import/export specifier), regardless of whether it's a plain
/// identifier or a string literal (`export { x as "some string" }`).
fn module_export_name(name: &ModuleExportName) -> (CompactStr, Span) {
    match name {
        ModuleExportName::IdentifierName(id) => (CompactStr::from(id.name.as_str()), id.span),
        ModuleExportName::IdentifierReference(id) => (CompactStr::from(id.name.as_str()), id.span),
        ModuleExportName::StringLiteral(lit) => (lit.value.to_compact_str(), lit.span),
    }
}
