//! ESLint-compatible `json` output format: a one-line JSON array with one entry per
//! linted file (even files with no diagnostics — ESLint's own behavior), enabling
//! existing ESLint-output consumers (CI parsers, reviewdog) to work unchanged.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use super::{OutputSeverity, RenderedDiagnostic};

#[derive(Serialize)]
struct Message<'a> {
    #[serde(rename = "ruleId")]
    rule_id: &'a str,
    /// ESLint severity encoding: `2` = error, `1` = warning.
    severity: u8,
    message: &'a str,
    #[serde(rename = "messageId")]
    message_id: &'a str,
    line: u32,
    column: u32,
    #[serde(rename = "endLine")]
    end_line: u32,
    #[serde(rename = "endColumn")]
    end_column: u32,
}

#[derive(Serialize)]
struct FileResult<'a> {
    #[serde(rename = "filePath")]
    file_path: String,
    messages: Vec<Message<'a>>,
    #[serde(rename = "errorCount")]
    error_count: usize,
    #[serde(rename = "warningCount")]
    warning_count: usize,
    #[serde(rename = "fixableErrorCount")]
    fixable_error_count: usize,
    #[serde(rename = "fixableWarningCount")]
    fixable_warning_count: usize,
}

/// Render one JSON array line to `out`: one [`FileResult`] per file in
/// `linted_files` (absolute `filePath`, sorted), each populated with the
/// diagnostics from `diagnostics` that belong to it.
pub fn render(
    out: &mut impl Write,
    diagnostics: &[RenderedDiagnostic],
    linted_files: &[PathBuf],
) -> io::Result<()> {
    let mut by_file: BTreeMap<String, Vec<&RenderedDiagnostic>> = BTreeMap::new();
    for file in linted_files {
        by_file
            .entry(file.to_string_lossy().into_owned())
            .or_default();
    }
    for diagnostic in diagnostics {
        by_file
            .entry(diagnostic.file.to_string_lossy().into_owned())
            .or_default()
            .push(diagnostic);
    }

    let results: Vec<FileResult> = by_file
        .into_iter()
        .map(|(file_path, file_diagnostics)| {
            let mut error_count = 0;
            let mut warning_count = 0;
            let messages = file_diagnostics
                .iter()
                .map(|diagnostic| {
                    let severity = match diagnostic.severity {
                        OutputSeverity::Error => {
                            error_count += 1;
                            2
                        }
                        OutputSeverity::Warn => {
                            warning_count += 1;
                            1
                        }
                    };
                    Message {
                        rule_id: diagnostic.rule_id,
                        severity,
                        message: &diagnostic.message,
                        message_id: &diagnostic.message_id,
                        line: diagnostic.line,
                        column: diagnostic.column,
                        end_line: diagnostic.end_line,
                        end_column: diagnostic.end_column,
                    }
                })
                .collect();
            FileResult {
                file_path,
                messages,
                error_count,
                warning_count,
                fixable_error_count: 0,
                fixable_warning_count: 0,
            }
        })
        .collect();

    let json = serde_json::to_string(&results).map_err(io::Error::other)?;
    writeln!(out, "{json}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_clean_files_and_counts_severities() {
        let clean = PathBuf::from("/proj/src/clean.ts");
        let dirty = PathBuf::from("/proj/src/dirty.ts");
        let diagnostics = vec![RenderedDiagnostic {
            file: dirty.clone(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 5,
            severity: OutputSeverity::Error,
            rule_id: "package-access",
            message: "boom".to_string(),
            message_id: "private".to_string(),
        }];
        let mut out = Vec::new();
        render(&mut out, &diagnostics, &[clean.clone(), dirty.clone()]).unwrap();
        let text = String::from_utf8(out).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let clean_entry = arr
            .iter()
            .find(|v| v["filePath"] == clean.to_string_lossy().as_ref())
            .unwrap();
        assert_eq!(clean_entry["messages"].as_array().unwrap().len(), 0);
        let dirty_entry = arr
            .iter()
            .find(|v| v["filePath"] == dirty.to_string_lossy().as_ref())
            .unwrap();
        assert_eq!(dirty_entry["errorCount"], 1);
        assert_eq!(dirty_entry["messages"][0]["severity"], 2);
        assert_eq!(dirty_entry["fixableErrorCount"], 0);
    }
}
