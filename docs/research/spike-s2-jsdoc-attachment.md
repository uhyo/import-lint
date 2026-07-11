# Spike S2 — JSDoc attachment coverage in oxc

> De-risks [PLAN-v1.md §10 spike S2](../PLAN-v1.md) and the extraction detail in §2.2. Verified
> empirically with a scratch `cargo` project against the pinned crate versions from
> [`oxc-ecosystem.md`](./oxc-ecosystem.md), 2026-07-10.

## Versions tested

`oxc_parser` `oxc_ast` `oxc_span` `oxc_allocator` `oxc_semantic` (feature `jsdoc`) `oxc_jsdoc` —
all **0.139.0**, resolved and locked via `cargo add`/`cargo build` from crates.io (no
substitutions). This matches the exact pins in `oxc-ecosystem.md` §1.

## Headline finding: `SemanticBuilder::new()` alone does NOT populate `Semantic::nodes()`

This is the one gap the research doc didn't call out and it will silently break any code that
does `SemanticBuilder::new().build(&program)` and then calls `semantic.nodes().iter()` or
`jsdoc_finder.get_*_by_node(...)` — **`Semantic::nodes()` comes back empty**, so
`get_all_by_node`/`get_one_by_node` always return `None`, for every node, with no error.

Cause: `oxc_semantic` 0.139.0 has two node-store modes (`crates/oxc_semantic/src/node/store.rs`,
`AstNodeStoreKind::{Full, Ancestry}`), and `with_build_nodes` **defaults to `false`**
(compiler-pipeline mode — only the lightweight ancestry stack is kept). The linter needs the full
store. Use the dedicated constructor:

```rust
let semantic = SemanticBuilder::new_linter().build(&program).semantic;
// equivalent to: SemanticBuilder::new().with_build_nodes(true).with_cfg(true)
//                    .with_class_table(true).with_check_syntax_error(true)
```

`import_lint`'s extraction code (§2.2, `extract/mod.rs`) must use `new_linter()` (or at minimum
`.with_build_nodes(true)`) — a plain `SemanticBuilder::new()` will make JSDoc extraction a silent
no-op across the whole extraction phase. This is worth a code comment or a debug-assertion at the
call site given how quietly it fails.

## Results table

| # | Form | Attached? | Node it attaches to | API |
|---|---|---|---|---|
| 1 | `export const x = 1;` | ✅ | `ExportNamedDeclaration` | `jsdoc.get_one_by_node` |
| 2 | `export function f() {}` | ✅ | `ExportNamedDeclaration` | same |
| 3 | `export class C {}` | ✅ | `ExportNamedDeclaration` | same |
| 4a | `export default 42;` | ✅ | `ExportDefaultDeclaration` | same |
| 4b | `export default function f() {}` | ✅ | `ExportDefaultDeclaration` | same |
| 5 | `export { x } from "./y";` | ✅ | `ExportNamedDeclaration` | same |
| 6 | `export { x };` (local) | ✅ | `ExportNamedDeclaration` | same |
| 7 | `export interface I {}` | ✅ | `ExportNamedDeclaration` | same |
| 8 | `export type T = string;` | ✅ | `ExportNamedDeclaration` | same |
| 9 | `export enum E {}` | ✅ | `ExportNamedDeclaration` | same |
| 10 | `export = foo;` (`TSExportAssignment`) | ❌ | — | **fallback required** |
| 11 | `declare module "pkg" { ... }` (`TSModuleDeclaration`, `.d.ts`) | ❌ | — | **fallback required** |

`@access package` tag body: ✅ parses correctly (see Tag parsing below).

## Why 1–9 all work, and why 10–11 don't: the exact mechanism

`oxc_jsdoc`'s builder (`oxc_jsdoc-0.139.0/src/builder.rs`) does **not** do nearest-ancestor
search at attachment time. It's a flat mechanism:

1. Every JSDoc-style block comment (`/** ... */`) already carries a precomputed
   `comment.attached_to: u32` — the byte offset of the very next token, i.e. the span-start of
   whatever statement/declaration follows it syntactically.
2. During the semantic AST walk, `oxc_semantic` calls `retrieve_attached_jsdoc(&kind)` for
   **every node it creates**, but only actually looks up a pending comment if `kind` matches a
   hardcoded allowlist, `should_attach_jsdoc()` (`oxc_jsdoc-0.139.0/src/builder.rs:149`):

   ```rust
   fn should_attach_jsdoc(kind: &AstKind) -> bool {
       matches!(kind,
           AstKind::BlockStatement(_) | AstKind::BreakStatement(_) | ... // control-flow statements
         | AstKind::VariableDeclaration(_) | AstKind::VariableDeclarator(_)
         | AstKind::ArrowFunctionExpression(_) | AstKind::ObjectExpression(_) | AstKind::ParenthesizedExpression(_)
         | AstKind::ObjectProperty(_)
         | AstKind::Function(_) | AstKind::FormalParameter(_)
         | AstKind::Class(_) | AstKind::MethodDefinition(_) | AstKind::PropertyDefinition(_) | AstKind::StaticBlock(_)
         | AstKind::Decorator(_)
         | AstKind::ExportAllDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
         | AstKind::ExportNamedDeclaration(_) | AstKind::ImportDeclaration(_)
           // NOTE: no TSInterfaceDeclaration / TSTypeAliasDeclaration / TSEnumDeclaration /
           //       TSModuleDeclaration / TSExportAssignment / TSImportEqualsDeclaration entries
       )
   }
   ```

3. If the current node's `kind` is in that list **and** its span-start exactly equals a pending
   comment's `attached_to`, the comment attaches to *that* node (keyed by span-start in
   `JSDocFinder`'s internal map) and is consumed (a `VariableDeclaration` nested one token later
   can never "steal" a comment already consumed by its wrapping `ExportNamedDeclaration`).

This explains every row in the table:

- **Forms 1–9**: in oxc's AST, `export interface`/`export type`/`export enum` are *not* distinct
  top-level node kinds — like `export const`/`function`/`class`, they're all
  `Declaration` variants boxed inside `ExportNamedDeclaration.declaration`
  (`oxc_ast-0.139.0/src/ast/js.rs:1132` `enum Declaration`). Since `export` is always the first
  token, `ExportNamedDeclaration`'s (or `ExportDefaultDeclaration`'s) span-start is the one that
  matches the comment's `attached_to`, and that wrapper kind **is** in the allowlist. So all nine
  forms attach — including `interface`/`type`/`enum`, contrary to the "test before relying" caveat
  in `oxc-ecosystem.md` (oxc issue #1506 concerns class methods/decorated properties, a different
  code path — not export declarations).
- **Form 10** (`export = foo;`): `TSExportAssignment` is its own top-level `ModuleDeclaration`
  variant (`oxc_ast-0.139.0/src/ast/js.rs:2387`), not wrapped by anything, and it is **absent**
  from `should_attach_jsdoc()`. No node ever matches the comment's position. Confirmed empirically
  (`get_all_by_node` returns `None` for every node in the tree).
- **Form 11** (`declare module "pkg" { ... }`): `TSModuleDeclaration` is a `Declaration` variant
  (`oxc_ast-0.139.0/src/ast/ts.rs:1239`), also **absent** from the allowlist, also unattached.
  (This applies whether or not the module declaration itself is `export`ed — there's no
  `export declare module` form in the exporter case we care about, since `.d.ts` ambient modules
  are written as bare `declare module "x" { ... }`, which is exactly form 11.)

## Nearest-ancestor / node-granularity question — answered

`JSDocFinder` does **not** do ancestor search at query time either — `get_all_by_node` is a direct
span-start dictionary lookup (`get_all_by_span(node.kind().span())`, keyed by `node.id()`'s flag
check first). So:

- You **must** query on the exact node that the builder attached to, not on an inner symbol node.
  For every exported declaration form, that is the wrapping `ExportNamedDeclaration` /
  `ExportDefaultDeclaration` node — never the inner `VariableDeclaration`, `Function`, `Class`,
  `TSInterfaceDeclaration`, etc., even though several of *those* kinds are separately in the
  allowlist (they only match when *not* wrapped in an export, i.e. a plain unexported
  `function f() {}`).
- This confirms PLAN-v1.md §2.2 is already stating the right target: "JSDoc association ... on the
  *statement* node that introduces each export (declaration statement, `ExportNamedDeclaration`,
  or `export default`)" — the extraction code should always look up JSDoc via the outermost
  `ModuleDeclaration`/`Declaration` node it is visiting for a given exported entity, not a nested
  binding/identifier node.
- Multiple stacked `/** */` blocks before one node **all** attach to that same node, in source
  (farthest→nearest) order via `get_all_by_node`; `get_one_by_node` returns `.last()` = nearest.
  Verified: `/** far */\n/** @package near */\nexport const x = 1;` → `get_all_by_node` returns
  `[far, "@package near"]` in that order; `get_one_by_node` returns the `"@package near"` one.
  This matters for the spec's "first match wins in source order" tag-scanning rule (§3.1) — if a
  statement has multiple stacked blocks, scan `get_all_by_node`'s tags in that farthest→nearest
  order (equivalently: just use `get_one_by_node`, since JS/TS convention is exactly one JSDoc
  block per declaration and the nearest one is what tools like TypeScript itself honor).

## Tag parsing — confirmed

`JSDocTag` (`oxc_jsdoc`) exposes:
- `.kind: JSDocTagKindPart` → `.parsed() -> &str`, the tag name **without** the `@`, e.g. `"package"`, `"private"`, `"public"`, `"access"`.
- `.comment() -> JSDocCommentPart` → `.parsed() -> String`, the trimmed text after the tag name.

Verified: `@access package` parses as `kind.parsed() == "access"`, `comment().parsed() == "package"` — exactly what's needed for the spec §3.1 `@access <level>` mapping. `@package` (no body) parses as `kind.parsed() == "package"`, empty comment.

## Primary API — minimal working snippet

```rust
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

let allocator = Allocator::default();
let source_type = SourceType::ts(); // or SourceType::d_ts() for .d.ts files
let ret = Parser::new(&allocator, source_text, source_type).parse();

// IMPORTANT: new_linter() (or .with_build_nodes(true)), not new() —
// see "headline finding" above.
let semantic = SemanticBuilder::new_linter().build(&ret.program).semantic;

let jsdoc_finder = semantic.jsdoc();
let nodes = semantic.nodes();

for node in nodes.iter() {
    if let Some(doc) = jsdoc_finder.get_one_by_node(nodes, node) {
        for tag in doc.tags() {
            let kind = tag.kind.parsed();      // "package" / "private" / "public" / "access"
            let body = tag.comment().parsed();  // e.g. "package" for `@access package`
            // first match of @package/@private/@public/@access <level> wins (spec §3.1)
        }
    }
}
```

In the real extractor you won't walk *all* nodes — you already have the specific
`ExportNamedDeclaration`/`ExportDefaultDeclaration`/(unexported)`Declaration` AST node in hand
from the module-record-driven visit (§2.2); call `get_one_by_node(nodes, that_node)` directly.

## Fallback API — minimal working snippet (needed for forms 10 and 11 only)

`Semantic::comments()` / `comments_range()` are real and exposed (`oxc_semantic-0.139.0/src/lib.rs:161-170`).
Fallback = nearest preceding `/** ... */` block comment with nothing but whitespace between its
end and the target statement's span-start:

```rust
use oxc_ast::ast::Comment;
use oxc_span::{GetSpan, Span};

fn fallback_jsdoc<'a>(source_text: &'a str, comments: &[Comment], stmt_span: Span) -> Option<&'a str> {
    comments
        .iter()
        .filter(|c| c.is_jsdoc() && c.span.end <= stmt_span.start)
        .filter(|c| source_text[c.span.end as usize..stmt_span.start as usize]
            .chars()
            .all(char::is_whitespace))
        .max_by_key(|c| c.span.start) // nearest = highest start among qualifying comments
        .map(|c| c.content_span().source_text(source_text)) // strips the leading `/**`'s `*`
}

// Usage: fallback_jsdoc(source_text, semantic.comments(), export_assignment_node.span())
```

Verified against both gap cases:
- `export = foo;` (preceded by `const foo = 1;`): `fallback_jsdoc` on the `TSExportAssignment`
  statement's span correctly returns the `@package` block, skipping the earlier unrelated
  statement.
- `declare module "pkg" { ... }`: returns the `@package` block directly (single top-level
  statement, trivial case).

Since the fallback needs the *tag parser*, not just the comment text, run the found raw comment
text through `oxc_jsdoc`'s tag scanner rather than hand-rolling `@tag` regex matching — either
call `oxc_jsdoc::JSDoc::new(comment_content, span)` directly (same constructor `JSDocBuilder` uses
internally, `oxc_jsdoc-0.139.0/src/builder.rs:137`) and then `.tags()`, or literally reuse
`JSDocBuilder::parse_jsdoc_comment`'s logic (it's a two-line span/`*`-strip operation, trivial to
inline) so both code paths produce identical `JSDocTag` values and the `Access`-mapping logic
never needs a second implementation.

## Recommendation

Use `Semantic::jsdoc()`/`JSDocFinder::get_one_by_node()` as the primary lookup for every exported
declaration form — it covers 9 of the 11 forms in spike scope (all `export const/function/class`,
`export default` expr/function, `export { x } [from "y"]` both with and without source,
`export interface/type/enum`) by querying the wrapping `ExportNamedDeclaration` /
`ExportDefaultDeclaration` node, never an inner declaration node. Add a manual
nearest-preceding-`/**`-comment fallback (via `Semantic::comments()`, feeding the matched text
through `oxc_jsdoc::JSDoc::new(...).tags()` for consistent tag parsing) for exactly two node
kinds that `oxc_jsdoc`'s hardcoded `should_attach_jsdoc()` allowlist omits: `TSExportAssignment`
(`export = foo;`) and `TSModuleDeclaration` (`declare module "x" { ... }`) — both are structurally
predictable (single top-level statement, or the statement following the last unrelated one), so
the fallback needs no general-purpose ancestor search, just the span-comparison shown above.
Wrap both paths behind one internal `fn jsdoc_for(node) -> Option<JSDoc>`-style API in
`extract/jsdoc.rs` so call sites in `extract/mod.rs` don't need to know which path fired.
Critically, **extraction must build `Semantic` via `SemanticBuilder::new_linter()`** (or explicit
`.with_build_nodes(true)`) — the plain `new()` silently returns an empty node store and every
`get_*_by_node` call becomes a no-op with no diagnostic.
