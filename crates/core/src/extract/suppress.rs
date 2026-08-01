//! Caller-side suppression directives: `import-lint-disable-next-line` and
//! `import-lint-disable-line` comments, the ImportLint equivalent of ESLint's
//! `eslint-disable-next-line` (which the reference plugin got for free from
//! ESLint itself).
//!
//! Syntax mirrors ESLint's directive comments:
//!
//! ```ts
//! // import-lint-disable-next-line
//! import { privateThing } from "./other-package/internal";
//!
//! import { privateThing } from "./other-package/internal"; // import-lint-disable-line
//!
//! // import-lint-disable-next-line package-access -- justification, ignored
//! import { privateThing } from "./other-package/internal";
//! ```
//!
//! A directive with no rule names suppresses every diagnostic on the target
//! line; with rule names (comma- and/or whitespace-separated), only the named
//! rules are suppressed. Everything after a ` -- ` separator is a free-form
//! justification and is ignored. Both `//` line comments and `/* ... */` block
//! comments work.
//!
//! Directives are collected at extraction time (the only phase that holds the
//! source text and its comments) into byte ranges stored on
//! [`FileModuleInfo::suppressions`](super::module_info::FileModuleInfo), so the
//! rule engine — and therefore every consumer: one-shot CLI, watch mode, LSP —
//! can filter diagnostics without re-reading the file.

use oxc_ast::ast::Comment;
use oxc_span::Span;
use oxc_str::CompactStr;

use super::module_info::Suppression;

/// Scan `comments` for suppression directives and turn each into the byte range
/// of the line it silences.
pub(crate) fn collect_suppressions(comments: &[Comment], source_text: &str) -> Vec<Suppression> {
    let mut out = Vec::new();
    for comment in comments {
        let content = comment.content_span().source_text(source_text);
        let Some((kind, rules)) = parse_directive(content) else {
            continue;
        };
        let span = match kind {
            // `disable-line` silences the line the comment starts on.
            DirectiveKind::Line => line_span_at(source_text, comment.span.start),
            // `disable-next-line` silences the line after the one the comment
            // ends on; a comment on the last line has no next line to silence.
            DirectiveKind::NextLine => {
                let Some(span) = next_line_span(source_text, comment.span.end) else {
                    continue;
                };
                span
            }
        };
        out.push(Suppression { span, rules });
    }
    out
}

enum DirectiveKind {
    Line,
    NextLine,
}

/// Parse one comment's content as a directive: the keyword (which must be the
/// first word), then optional rule names, then an optional ` -- justification`.
/// Returns `None` for anything that isn't a directive comment.
fn parse_directive(content: &str) -> Option<(DirectiveKind, Vec<CompactStr>)> {
    let trimmed = content.trim_start();
    // `-next-line` first: `import-lint-disable-line` is not a prefix of it, but
    // checking the shorter keyword first would still be wrong for any future
    // keyword pair where one IS a prefix of the other.
    let (kind, rest) = if let Some(rest) = trimmed.strip_prefix("import-lint-disable-next-line") {
        (DirectiveKind::NextLine, rest)
    } else {
        let rest = trimmed.strip_prefix("import-lint-disable-line")?;
        (DirectiveKind::Line, rest)
    };
    // The keyword must be a whole word (`import-lint-disable-liner` is not a
    // directive), delimited by whitespace or the end of the comment.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let mut rules = Vec::new();
    for token in rest.split_whitespace() {
        // ` -- reason` terminates the rule list, like ESLint's justification
        // separator.
        if token.starts_with("--") {
            break;
        }
        // Rule names may be separated by commas with or without spaces:
        // `a, b`, `a,b`, and `a b` all parse to `[a, b]`.
        for name in token.split(',').filter(|name| !name.is_empty()) {
            rules.push(CompactStr::from(name));
        }
    }
    Some((kind, rules))
}

/// Byte span of the line containing `offset`: from just after the preceding
/// newline to the following newline (exclusive) or end of input.
fn line_span_at(source_text: &str, offset: u32) -> Span {
    let offset = (offset as usize).min(source_text.len());
    let start = source_text[..offset]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = source_text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .unwrap_or(source_text.len());
    Span::new(start as u32, end as u32)
}

/// Byte span of the line after the one containing `offset`, or `None` if that
/// line doesn't exist (offset is on the last line).
fn next_line_span(source_text: &str, offset: u32) -> Option<Span> {
    let offset = (offset as usize).min(source_text.len());
    let start = source_text[offset..].find('\n')? + offset + 1;
    let end = source_text[start..]
        .find('\n')
        .map(|i| start + i)
        .unwrap_or(source_text.len());
    Some(Span::new(start as u32, end as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Option<(bool, Vec<CompactStr>)> {
        parse_directive(content)
            .map(|(kind, rules)| (matches!(kind, DirectiveKind::NextLine), rules))
    }

    #[test]
    fn bare_directives_parse_with_no_rules() {
        assert_eq!(
            parse(" import-lint-disable-next-line"),
            Some((true, vec![]))
        );
        assert_eq!(parse(" import-lint-disable-line"), Some((false, vec![])));
        assert_eq!(
            parse("import-lint-disable-next-line "),
            Some((true, vec![]))
        );
    }

    #[test]
    fn non_directives_are_ignored() {
        assert_eq!(parse(" just a comment"), None);
        assert_eq!(parse(" import-lint-disable"), None);
        assert_eq!(parse(" import-lint-disable-liner"), None);
        assert_eq!(parse(" import-lint-disable-next-lines"), None);
        assert_eq!(parse(" note: import-lint-disable-next-line"), None);
    }

    #[test]
    fn rule_names_parse_comma_or_space_separated() {
        let rules = vec![CompactStr::from("a"), CompactStr::from("b")];
        assert_eq!(
            parse(" import-lint-disable-next-line a, b"),
            Some((true, rules.clone()))
        );
        assert_eq!(
            parse(" import-lint-disable-next-line a,b"),
            Some((true, rules.clone()))
        );
        assert_eq!(
            parse(" import-lint-disable-next-line a b"),
            Some((true, rules))
        );
    }

    #[test]
    fn justification_after_dashes_is_ignored() {
        assert_eq!(
            parse(" import-lint-disable-next-line -- legacy, will fix in #123"),
            Some((true, vec![]))
        );
        assert_eq!(
            parse(" import-lint-disable-next-line package-access -- reason"),
            Some((true, vec![CompactStr::from("package-access")]))
        );
    }

    #[test]
    fn line_span_at_covers_the_whole_line() {
        let src = "aaa\nbbb\nccc";
        assert_eq!(line_span_at(src, 5), Span::new(4, 7));
        assert_eq!(line_span_at(src, 0), Span::new(0, 3));
        assert_eq!(line_span_at(src, 9), Span::new(8, 11));
    }

    #[test]
    fn next_line_span_covers_the_following_line() {
        let src = "aaa\nbbb\nccc";
        assert_eq!(next_line_span(src, 1), Some(Span::new(4, 7)));
        assert_eq!(next_line_span(src, 5), Some(Span::new(8, 11)));
        // Last line: nothing follows.
        assert_eq!(next_line_span(src, 9), None);
    }

    #[test]
    fn suppression_matches_offset_and_rule() {
        let bare = Suppression {
            span: Span::new(4, 7),
            rules: vec![],
        };
        assert!(bare.suppresses(4, "package-access"));
        assert!(bare.suppresses(6, "anything"));
        assert!(!bare.suppresses(7, "package-access"));
        assert!(!bare.suppresses(3, "package-access"));

        let named = Suppression {
            span: Span::new(4, 7),
            rules: vec![CompactStr::from("package-access")],
        };
        assert!(named.suppresses(5, "package-access"));
        assert!(!named.suppresses(5, "unresolved"));
    }
}
