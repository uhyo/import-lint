//! `github` output format: one GitHub Actions workflow command per diagnostic
//! (`::error file=...,line=...,col=...,endLine=...,endColumn=...::message`, or
//! `::warning` for warn severity).

use std::io::{self, Write};
use std::path::Path;

use super::{OutputSeverity, RenderedDiagnostic};

/// Render one workflow-command line per diagnostic to `out`. `cwd` is used to
/// display file paths relative to it (with forward slashes, per GitHub's format).
pub fn render(
    out: &mut impl Write,
    diagnostics: &[RenderedDiagnostic],
    cwd: &Path,
) -> io::Result<()> {
    for diagnostic in diagnostics {
        let level = match diagnostic.severity {
            OutputSeverity::Error => "error",
            OutputSeverity::Warn => "warning",
        };
        let display = diagnostic
            .file
            .strip_prefix(cwd)
            .unwrap_or(&diagnostic.file);
        let path = display.to_string_lossy().replace('\\', "/");
        writeln!(
            out,
            "::{level} file={path},line={},col={},endLine={},endColumn={}::{}",
            diagnostic.line,
            diagnostic.column,
            diagnostic.end_line,
            diagnostic.end_column,
            diagnostic.message,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn renders_error_and_warning_lines() {
        let diagnostics = vec![
            RenderedDiagnostic {
                file: PathBuf::from("/proj/src/a.ts"),
                line: 3,
                column: 10,
                end_line: 3,
                end_column: 20,
                severity: OutputSeverity::Error,
                rule_id: "package-access",
                message: "Cannot import ...".to_string(),
                message_id: "private".to_string(),
            },
            RenderedDiagnostic {
                file: PathBuf::from("/proj/src/b.ts"),
                line: 5,
                column: 1,
                end_line: 5,
                end_column: 10,
                severity: OutputSeverity::Warn,
                rule_id: "import-access/unresolved",
                message: "Unresolved import specifier './gone'".to_string(),
                message_id: "unresolved".to_string(),
            },
        ];
        let mut out = Vec::new();
        render(&mut out, &diagnostics, Path::new("/proj")).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(
            "::error file=src/a.ts,line=3,col=10,endLine=3,endColumn=20::Cannot import ..."
        ));
        assert!(text.contains("::warning file=src/b.ts,line=5,col=1,endLine=5,endColumn=10"));
    }
}
