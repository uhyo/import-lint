//! Rule options for the `jsdoc` rule (spec §4, M3), deserialized from a project
//! manifest's camelCase options object. Mirrors the reference plugin's
//! `RuleOptions` shape exactly — field names, defaults, and casing.

use serde::Deserialize;

/// JSDoc-declared (or default) access level, as accepted in the `defaultImportability`
/// option. Distinct from [`crate::extract::Access`] only in that it's the
/// deserialization target for user-facing option values; the rule engine converts
/// between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importability {
    Public,
    Package,
    Private,
}

/// How a bare specifier matching the importer's own package name should be
/// classified, as accepted in the `treatSelfReferenceAs` option (spec §4.6). Carried
/// on [`JsdocRuleOptions`] so a single struct deserializes the reference options
/// object, but consumed by the resolver (`SelfReferenceMode`) — the check phase
/// itself ignores this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelfRefOpt {
    Internal,
    External,
}

/// Options for the `jsdoc` rule (spec §4). Deserializes the exact camelCase option
/// names the reference plugin accepts: `indexLoophole`, `filenameLoophole`,
/// `defaultImportability`, `treatSelfReferenceAs`, `excludeSourcePatterns`,
/// `packageDirectory`. `deny_unknown_fields` is typo protection for the config file
/// (M5): a misspelled option name is a hard load error rather than a silently
/// ignored no-op. This is also what makes `config::JsdocRuleConfig`'s
/// `#[serde(flatten)]` field reject unknown keys, since serde disallows
/// `deny_unknown_fields` on a struct that itself has a flatten field — the
/// flattened type is where that check has to live instead.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct JsdocRuleOptions {
    pub index_loophole: bool,
    pub filename_loophole: bool,
    pub default_importability: Importability,
    pub treat_self_reference_as: SelfRefOpt,
    pub exclude_source_patterns: Vec<String>,
    pub package_directory: Option<Vec<String>>,
}

impl Default for JsdocRuleOptions {
    fn default() -> Self {
        Self {
            index_loophole: true,
            filename_loophole: false,
            default_importability: Importability::Public,
            treat_self_reference_as: SelfRefOpt::External,
            exclude_source_patterns: Vec::new(),
            package_directory: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_documented_defaults() {
        let opts = JsdocRuleOptions::default();
        assert!(opts.index_loophole);
        assert!(!opts.filename_loophole);
        assert_eq!(opts.default_importability, Importability::Public);
        assert_eq!(opts.treat_self_reference_as, SelfRefOpt::External);
        assert!(opts.exclude_source_patterns.is_empty());
        assert!(opts.package_directory.is_none());
    }

    #[test]
    fn deserializes_camel_case_option_names() {
        let json = serde_json::json!({
            "indexLoophole": false,
            "filenameLoophole": true,
            "defaultImportability": "package",
            "treatSelfReferenceAs": "internal",
            "excludeSourcePatterns": ["src/**"],
            "packageDirectory": ["**"],
        });
        let opts: JsdocRuleOptions = serde_json::from_value(json).unwrap();
        assert!(!opts.index_loophole);
        assert!(opts.filename_loophole);
        assert_eq!(opts.default_importability, Importability::Package);
        assert_eq!(opts.treat_self_reference_as, SelfRefOpt::Internal);
        assert_eq!(opts.exclude_source_patterns, vec!["src/**".to_string()]);
        assert_eq!(opts.package_directory, Some(vec!["**".to_string()]));
    }

    #[test]
    fn partial_options_object_falls_back_to_defaults() {
        let json = serde_json::json!({ "defaultImportability": "private" });
        let opts: JsdocRuleOptions = serde_json::from_value(json).unwrap();
        assert!(opts.index_loophole);
        assert_eq!(opts.default_importability, Importability::Private);
    }
}
