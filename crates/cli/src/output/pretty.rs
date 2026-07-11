//! The default `pretty` output format: ESLint-stylish-like, one file header per
//! group of diagnostics followed by an indented `line:col  severity  message  rule`
//! line each, ending in a `✖ N problems (...)` summary. Nothing is printed for a
//! clean run (matching ESLint, which prints nothing rather than a "no problems"
//! banner).

use std::io::{self, Write};
use std::path::Path;

use super::{OutputSeverity, RenderedDiagnostic};

const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Render `diagnostics` (already sorted by `(file, line, column)`) to `out`. `cwd`
/// is used to display file paths relative to it when possible. `colors` gates ANSI
/// escapes — callers should pass `false` when stdout isn't a TTY.
pub fn render(
    out: &mut impl Write,
    diagnostics: &[RenderedDiagnostic],
    cwd: &Path,
    colors: bool,
) -> io::Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }

    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    let mut current_file: Option<&Path> = None;

    for diagnostic in diagnostics {
        if current_file != Some(diagnostic.file.as_path()) {
            if current_file.is_some() {
                writeln!(out)?;
            }
            let display = diagnostic
                .file
                .strip_prefix(cwd)
                .unwrap_or(&diagnostic.file);
            if colors {
                writeln!(out, "{BOLD}{}{RESET}", display.display())?;
            } else {
                writeln!(out, "{}", display.display())?;
            }
            current_file = Some(diagnostic.file.as_path());
        }

        let severity_word = match diagnostic.severity {
            OutputSeverity::Error => {
                error_count += 1;
                "error"
            }
            OutputSeverity::Warn => {
                warning_count += 1;
                "warning"
            }
        };
        let severity_display = if colors {
            let color = match diagnostic.severity {
                OutputSeverity::Error => RED,
                OutputSeverity::Warn => YELLOW,
            };
            format!("{color}{severity_word}{RESET}")
        } else {
            severity_word.to_string()
        };
        let rule_display = if colors {
            format!("{DIM}{}{RESET}", diagnostic.rule_id)
        } else {
            diagnostic.rule_id.to_string()
        };

        writeln!(
            out,
            "  {}:{}  {severity_display}  {}  {rule_display}",
            diagnostic.line, diagnostic.column, diagnostic.message,
        )?;
    }

    writeln!(out)?;
    let total = error_count + warning_count;
    let summary = format!(
        "\u{2716} {total} problem{} ({error_count} error{}, {warning_count} warning{})",
        plural(total),
        plural(error_count),
        plural(warning_count),
    );
    if colors {
        writeln!(out, "{RED}{summary}{RESET}")?;
    } else {
        writeln!(out, "{summary}")?;
    }
    Ok(())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn diag(file: &str, severity: OutputSeverity) -> RenderedDiagnostic {
        RenderedDiagnostic {
            file: PathBuf::from(file),
            line: 3,
            column: 10,
            end_line: 3,
            end_column: 20,
            severity,
            rule_id: "import-access/jsdoc",
            message: "Cannot import a package-private export 'x'".to_string(),
            message_id: "package".to_string(),
        }
    }

    #[test]
    fn empty_diagnostics_prints_nothing() {
        let mut out = Vec::new();
        render(&mut out, &[], Path::new("/proj"), false).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn plain_output_has_no_ansi_and_contains_expected_fields() {
        let diagnostics = vec![diag("/proj/src/a.ts", OutputSeverity::Error)];
        let mut out = Vec::new();
        render(&mut out, &diagnostics, Path::new("/proj"), false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains('\x1b'));
        assert!(text.contains("src/a.ts"));
        assert!(text.contains("3:10"));
        assert!(text.contains("Cannot import a package-private export 'x'"));
        assert!(text.contains("import-access/jsdoc"));
        assert!(text.contains("1 problem (1 error, 0 warnings)"));
    }

    #[test]
    fn colored_output_has_ansi_codes() {
        let diagnostics = vec![diag("/proj/src/a.ts", OutputSeverity::Warn)];
        let mut out = Vec::new();
        render(&mut out, &diagnostics, Path::new("/proj"), true).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains('\x1b'));
    }
}
