//! JSDoc tag -> [`Access`] resolution (spec §3.1).
//!
//! `oxc_jsdoc`'s `JSDocFinder` attaches comments to only a hardcoded allowlist of AST
//! node kinds (`should_attach_jsdoc()` in `oxc_jsdoc`'s builder) — every export form we
//! care about is on that list *except* `TSExportAssignment` (`export = x;`) and
//! `TSModuleDeclaration` (`declare module "x" { ... }`), which need a manual
//! nearest-preceding-comment fallback. See docs/research/spike-s2-jsdoc-attachment.md.

use oxc_ast::ast::Comment;
use oxc_jsdoc::JSDoc;
use oxc_semantic::Semantic;
use oxc_span::Span;

use super::module_info::Access;

/// Resolve the effective `Access` for the statement occupying `span`, or `None` if
/// there is no JSDoc at all, or none of its tags are recognized.
///
/// Tries `Semantic::jsdoc()` first; this is a plain span-keyed lookup
/// (`get_all_by_span`, not `get_all_by_node`) so it works regardless of whether the
/// full `AstNodes` store was built. Falling back to a manual comment scan is always
/// safe to attempt unconditionally: if a JSDoc comment WAS attached to `span` by the
/// primary path, that path already returned `Some` and the fallback is never reached,
/// so it can never "steal" or double-count a comment that oxc's finder already claimed.
pub(crate) fn access_for_span<'a>(
    semantic: &Semantic<'a>,
    source_text: &'a str,
    span: Span,
) -> Option<Access> {
    if let Some(docs) = semantic.jsdoc().get_all_by_span(span) {
        return access_from_jsdocs(docs.iter());
    }
    let fallback = nearest_preceding_jsdoc(source_text, semantic.comments(), span)?;
    access_from_jsdocs(std::iter::once(&fallback))
}

/// Manual fallback (spike S2): the nearest preceding `/** ... */` block comment with
/// nothing but whitespace between its end and `stmt_span`'s start. Only needed for
/// `export =` and `declare module` statements, but harmless to run for anything else
/// (see doc comment on [`access_for_span`]).
fn nearest_preceding_jsdoc<'a>(
    source_text: &'a str,
    comments: &[Comment],
    stmt_span: Span,
) -> Option<JSDoc<'a>> {
    comments
        .iter()
        .filter(|c| c.is_jsdoc() && c.span.end <= stmt_span.start)
        .filter(|c| {
            source_text[c.span.end as usize..stmt_span.start as usize]
                .chars()
                .all(char::is_whitespace)
        })
        .max_by_key(|c| c.span.start)
        .map(|c| {
            // `content_span()` strips the `/*`/`*/` delimiters; strip the JSDoc's
            // leading `*` too, matching `JSDocBuilder::parse_jsdoc_comment`.
            let content_span = c.content_span();
            let jsdoc_span = Span::new(content_span.start + 1, content_span.end);
            JSDoc::new(jsdoc_span.source_text(source_text), jsdoc_span)
        })
}

/// Scan tags across possibly-multiple stacked JSDoc blocks, farthest-to-nearest (i.e.
/// in source order, matching how `get_all_by_span` orders them), returning the first
/// recognized tag per spec §3.1: `@package` / `@private` / `@public` / `@access
/// <level>`. Unrecognized tags fall through.
fn access_from_jsdocs<'a: 'b, 'b>(docs: impl Iterator<Item = &'b JSDoc<'a>>) -> Option<Access> {
    for doc in docs {
        for tag in doc.tags() {
            let access = match tag.kind.parsed() {
                "package" => Some(Access::Package),
                "private" => Some(Access::Private),
                "public" => Some(Access::Public),
                "access" => match tag.comment().parsed().trim() {
                    "public" => Some(Access::Public),
                    "package" => Some(Access::Package),
                    "private" => Some(Access::Private),
                    _ => None,
                },
                _ => None,
            };
            if access.is_some() {
                return access;
            }
        }
    }
    None
}
