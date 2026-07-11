//! `.importlintrc.jsonc` / `.importlintrc.json` config model, discovery, and jsonc
//! loading (PLAN.md §4, D7, M5).
//!
//! Project root = the directory containing the config file (fallback: the caller's
//! cwd when no config file exists); `include`/`exclude`/`tsconfig` are all
//! interpreted relative to it by the CLI crate, not by this module.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use jsonc_parser::ParseOptions;
use serde::Deserialize;

use crate::rule::JsdocRuleOptions;

/// The `.importlintrc.jsonc` file names checked by [`find_config`], in priority
/// order (jsonc wins over json in the same directory, D7).
const CONFIG_FILE_NAMES: [&str; 2] = [".importlintrc.jsonc", ".importlintrc.json"];

/// Top-level config file shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct LintConfig {
    /// Roots to walk for lint targets, relative to the project root. Default: `["."]`.
    pub include: Vec<String>,
    /// Globs excluded from discovery, in addition to `.gitignore`, relative to the
    /// project root. Default: `[]`.
    pub exclude: Vec<String>,
    /// Path to the project's `tsconfig.json`, relative to the project root.
    pub tsconfig: Option<PathBuf>,
    pub rules: Rules,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            include: vec![".".to_string()],
            exclude: Vec::new(),
            tsconfig: None,
            rules: Rules::default(),
        }
    }
}

/// The `rules` map. A map (rather than a single option block) keeps the door open
/// for future rules without a config-shape break (PLAN.md §4).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Rules {
    pub jsdoc: JsdocRuleConfig,
}

/// The `jsdoc` rule's config entry: its severity plus its options, deserialized
/// from the same JSON object (`{"severity": "warn", "indexLoophole": false, ...}`).
#[derive(Debug, Clone, Default)]
pub struct JsdocRuleConfig {
    pub severity: Severity,
    pub options: JsdocRuleOptions,
}

/// Deserialization helper: `#[serde(flatten)]` cannot be combined with
/// `#[serde(deny_unknown_fields)]` on the same struct — serde silently *disables*
/// unknown-field detection for the whole struct in that combination rather than
/// erroring at either compile or run time (a known serde limitation; see
/// serde-rs/serde#1600). So `severity` is parsed as its own named field here, and
/// every other key is captured into `extra` as a generic JSON object, which then
/// gets deserialized into `JsdocRuleOptions` as a completely separate (non-flatten)
/// pass — an ordinary `Deserialize` call, where `deny_unknown_fields` works exactly
/// as documented.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawJsdocRuleConfig {
    severity: Severity,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for JsdocRuleConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawJsdocRuleConfig::deserialize(deserializer)?;
        let options: JsdocRuleOptions =
            serde_json::from_value(serde_json::Value::Object(raw.extra))
                .map_err(serde::de::Error::custom)?;
        Ok(JsdocRuleConfig {
            severity: raw.severity,
            options,
        })
    }
}

/// A rule's configured severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Error,
    Warn,
    Off,
}

/// A config file failed to load: it couldn't be read, or its contents failed to
/// parse as jsonc/deserialize into [`LintConfig`]. `path` and `message` are combined
/// by [`fmt::Display`] into a single line suitable for a CLI error exit.
#[derive(Debug)]
pub struct ConfigError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for ConfigError {}

impl LintConfig {
    /// Load and parse a config file at `path`. Missing-file handling is the
    /// caller's job (D7: a missing config file means `LintConfig::default()`) —
    /// this function always attempts to read `path` and errors if it can't.
    pub fn load(path: &Path) -> Result<LintConfig, ConfigError> {
        let text = fs::read_to_string(path).map_err(|err| ConfigError {
            path: path.to_path_buf(),
            message: format!("cannot read config file: {err}"),
        })?;
        jsonc_parser::parse_to_serde_value::<LintConfig>(&text, &ParseOptions::default()).map_err(
            |err| ConfigError {
                path: path.to_path_buf(),
                message: err.to_string(),
            },
        )
    }
}

/// Walk upward from `start_dir` (inclusive) to the filesystem root, returning the
/// first `.importlintrc.jsonc` or `.importlintrc.json` found (jsonc wins when both
/// exist in the same directory, D7). Returns `None` if neither is found anywhere up
/// the tree.
pub fn find_config(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = Some(start_dir);
    while let Some(d) = dir {
        for name in CONFIG_FILE_NAMES {
            let candidate = d.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{Importability, SelfRefOpt};
    use tempfile::TempDir;

    fn write(dir: &Path, relative: &str, contents: &str) -> PathBuf {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn defaults() {
        let config = LintConfig::default();
        assert_eq!(config.include, vec!["."]);
        assert!(config.exclude.is_empty());
        assert!(config.tsconfig.is_none());
        assert_eq!(config.rules.jsdoc.severity, Severity::Error);
        assert!(config.rules.jsdoc.options.index_loophole);
    }

    #[test]
    fn parses_jsonc_comments_and_trailing_commas() {
        let dir = TempDir::new().unwrap();
        let path = write(
            dir.path(),
            ".importlintrc.jsonc",
            r#"{
                // a comment
                "include": ["src"],
                "rules": {
                    "jsdoc": {
                        "severity": "warn",
                        "indexLoophole": false, // trailing comma below
                    },
                },
            }"#,
        );
        let config = LintConfig::load(&path).expect("should parse");
        assert_eq!(config.include, vec!["src"]);
        assert_eq!(config.rules.jsdoc.severity, Severity::Warn);
        assert!(!config.rules.jsdoc.options.index_loophole);
    }

    #[test]
    fn full_option_set_deserializes() {
        let dir = TempDir::new().unwrap();
        let path = write(
            dir.path(),
            ".importlintrc.json",
            r#"{
                "include": ["src"],
                "exclude": ["**/dist/**"],
                "tsconfig": "./tsconfig.json",
                "rules": {
                    "jsdoc": {
                        "severity": "off",
                        "indexLoophole": false,
                        "filenameLoophole": true,
                        "defaultImportability": "package",
                        "treatSelfReferenceAs": "internal",
                        "excludeSourcePatterns": ["**/*.gen.ts"],
                        "packageDirectory": ["**"]
                    }
                }
            }"#,
        );
        let config = LintConfig::load(&path).expect("should parse");
        assert_eq!(config.exclude, vec!["**/dist/**"]);
        assert_eq!(config.tsconfig, Some(PathBuf::from("./tsconfig.json")));
        assert_eq!(config.rules.jsdoc.severity, Severity::Off);
        assert!(!config.rules.jsdoc.options.index_loophole);
        assert!(config.rules.jsdoc.options.filename_loophole);
        assert_eq!(
            config.rules.jsdoc.options.default_importability,
            Importability::Package
        );
        assert_eq!(
            config.rules.jsdoc.options.treat_self_reference_as,
            SelfRefOpt::Internal
        );
        assert_eq!(
            config.rules.jsdoc.options.exclude_source_patterns,
            vec!["**/*.gen.ts".to_string()]
        );
        assert_eq!(
            config.rules.jsdoc.options.package_directory,
            Some(vec!["**".to_string()])
        );
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = write(
            dir.path(),
            ".importlintrc.jsonc",
            r#"{ "includ": ["src"] }"#,
        );
        let err = LintConfig::load(&path).expect_err("should reject typo'd field");
        assert!(err.to_string().contains("includ"), "error was: {err}");
        assert!(err.to_string().contains(".importlintrc.jsonc"));
    }

    #[test]
    fn unknown_rule_option_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = write(
            dir.path(),
            ".importlintrc.jsonc",
            r#"{ "rules": { "jsdoc": { "indexLoophol": false } } }"#,
        );
        let err = LintConfig::load(&path).expect_err("should reject typo'd option");
        assert!(err.to_string().contains("indexLoophol"), "error was: {err}");
    }

    #[test]
    fn unknown_rule_name_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = write(
            dir.path(),
            ".importlintrc.jsonc",
            r#"{ "rules": { "jsdco": {} } }"#,
        );
        let err = LintConfig::load(&path).expect_err("should reject typo'd rule name");
        assert!(err.to_string().contains("jsdco"), "error was: {err}");
    }

    #[test]
    fn missing_file_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does-not-exist.jsonc");
        let err = LintConfig::load(&path).expect_err("should error on missing file");
        assert!(err.to_string().contains("does-not-exist.jsonc"));
    }

    #[test]
    fn find_config_walks_upward() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), ".importlintrc.jsonc", "{}");
        let nested = dir.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        let found = find_config(&nested).expect("should find config walking up");
        assert_eq!(found, dir.path().join(".importlintrc.jsonc"));
    }

    #[test]
    fn find_config_returns_none_when_absent() {
        // A tempdir has no ancestor `.importlintrc.*` (assuming CI/dev boxes don't
        // have one at `/`), so this should walk all the way up and find nothing.
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();

        // Only assert None if there truly isn't one anywhere above the tempdir
        // (defensive: some exotic CI env could have one at `/tmp` or similar).
        if find_config(std::path::Path::new("/")).is_none() {
            assert!(find_config(&nested).is_none());
        }
    }

    #[test]
    fn jsonc_wins_over_json_in_same_directory() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            ".importlintrc.json",
            r#"{ "include": ["json"] }"#,
        );
        write(
            dir.path(),
            ".importlintrc.jsonc",
            r#"{ "include": ["jsonc"] }"#,
        );

        let found = find_config(dir.path()).expect("should find a config");
        assert_eq!(found, dir.path().join(".importlintrc.jsonc"));
    }
}
