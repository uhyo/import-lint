//! Diagnostics produced by the rule engine (M3) and the line/column conversion
//! helper used to render them (`Span` byte offsets -> 1-based line/column, ESLint
//! convention: column counts UTF-16 code units since the line start, then +1).

use std::path::PathBuf;

use oxc_span::Span;
use oxc_str::CompactStr;

/// Which of the four messages a diagnostic renders (spec §4: `package`/`private`,
/// each with a `:reexport` variant used when the checked entry is a re-export rather
/// than a plain import).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageId {
    Package,
    PackageReexport,
    Private,
    PrivateReexport,
}

impl MessageId {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageId::Package => "package",
            MessageId::PackageReexport => "package:reexport",
            MessageId::Private => "private",
            MessageId::PrivateReexport => "private:reexport",
        }
    }
}

/// One reported violation: an import or re-export site whose target's access level
/// forbids it.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// The importer file (absolute path) — where the checked entry lives.
    pub path: PathBuf,
    /// Span of the checked entry (the whole specifier node, including any `as
    /// alias` suffix), in the importer's source text.
    pub span: Span,
    pub message_id: MessageId,
    /// The name being imported/re-exported, as it appears in the rendered message.
    pub identifier: CompactStr,
}

impl Diagnostic {
    /// Render this diagnostic's message text exactly as the reference plugin does.
    pub fn message(&self) -> String {
        match self.message_id {
            MessageId::Package => {
                format!(
                    "Cannot import a package-private export '{}'",
                    self.identifier
                )
            }
            MessageId::PackageReexport => format!(
                "Cannot re-export a package-private export '{}'",
                self.identifier
            ),
            MessageId::Private => {
                format!("Cannot import a private export '{}'", self.identifier)
            }
            MessageId::PrivateReexport => {
                format!("Cannot re-export a private export '{}'", self.identifier)
            }
        }
    }
}

/// Convert a byte offset into `source` to a 1-based `(line, column)` pair, ESLint
/// convention: `column` counts UTF-16 code units since the start of the line, then
/// adds 1 (so the first column on a line is `1`, not `0`).
///
/// `byte_offset` is assumed to land on a UTF-8 character boundary (true for every
/// span oxc produces); a `byte_offset` past the end of `source` clamps to the end.
pub fn line_col(source: &str, byte_offset: u32) -> (u32, u32) {
    let byte_offset = (byte_offset as usize).min(source.len());
    let prefix = &source[..byte_offset];

    let line = 1 + prefix.bytes().filter(|&b| b == b'\n').count() as u32;

    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = 1 + source[line_start..byte_offset].encode_utf16().count() as u32;

    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_rendering() {
        let base = Diagnostic {
            path: PathBuf::from("/a.ts"),
            span: Span::new(0, 1),
            message_id: MessageId::Package,
            identifier: CompactStr::from("x"),
        };
        assert_eq!(base.message(), "Cannot import a package-private export 'x'");

        let mut d = base.clone();
        d.message_id = MessageId::PackageReexport;
        assert_eq!(d.message(), "Cannot re-export a package-private export 'x'");

        let mut d = base.clone();
        d.message_id = MessageId::Private;
        assert_eq!(d.message(), "Cannot import a private export 'x'");

        let mut d = base.clone();
        d.message_id = MessageId::PrivateReexport;
        assert_eq!(d.message(), "Cannot re-export a private export 'x'");
    }

    #[test]
    fn message_id_as_str() {
        assert_eq!(MessageId::Package.as_str(), "package");
        assert_eq!(MessageId::PackageReexport.as_str(), "package:reexport");
        assert_eq!(MessageId::Private.as_str(), "private");
        assert_eq!(MessageId::PrivateReexport.as_str(), "private:reexport");
    }

    #[test]
    fn line_col_offset_zero_is_line_one_column_one() {
        assert_eq!(line_col("abc", 0), (1, 1));
    }

    #[test]
    fn line_col_ascii_multiline() {
        let source = "abc\ndef\nghi";
        // 'd' is byte offset 4, start of line 2.
        assert_eq!(line_col(source, 4), (2, 1));
        // 'f' is byte offset 6, third column of line 2.
        assert_eq!(line_col(source, 6), (2, 3));
        // offset at the very start of line 3.
        assert_eq!(line_col(source, 8), (3, 1));
    }

    #[test]
    fn line_col_at_line_start_after_newline() {
        let source = "x\ny";
        assert_eq!(line_col(source, 2), (2, 1));
    }

    #[test]
    fn line_col_counts_utf16_code_units_not_bytes() {
        // "é" is 2 bytes in UTF-8 but 1 UTF-16 code unit; "🙂" is 4 bytes in UTF-8
        // but a UTF-16 surrogate pair (2 code units).
        let source = "é🙂x";
        // byte offsets: é=0..2, 🙂=2..6, x=6..7
        assert_eq!(line_col(source, 0), (1, 1));
        assert_eq!(line_col(source, 2), (1, 2)); // after é: 1 UTF-16 unit -> column 2
        assert_eq!(line_col(source, 6), (1, 4)); // after 🙂: +2 UTF-16 units -> column 4
        assert_eq!(line_col(source, 7), (1, 5));
    }

    #[test]
    fn line_col_clamps_offset_past_end() {
        let source = "abc";
        assert_eq!(line_col(source, 100), (1, 4));
    }
}
