//! Rule options for the `package-access` rule (spec §4, M3), deserialized from a
//! project manifest's camelCase options object. Mirrors the reference plugin's
//! `RuleOptions` shape exactly — field names, defaults, and casing.

use std::collections::HashMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};

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
/// on [`PackageAccessRuleOptions`] so a single struct deserializes the reference
/// options object, but consumed by the resolver (`SelfReferenceMode`) — the check
/// phase itself ignores this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelfRefOpt {
    Internal,
    External,
}

/// The `nonTsFiles` option: access levels for the exports of non-TS files (files
/// ImportLint resolves but cannot parse as modules — CSS modules, JSON, SVG, ...).
///
/// Config shape: a JSON object whose keys are glob patterns matched against the
/// exporting file's *resolved* path relative to the project root (not the import
/// specifier), and whose values map an export name to an access level. The key
/// `"*"` matches any export name *except* `default` (following the ES spec's
/// `export *` convention, which never forwards `default`):
///
/// ```jsonc
/// "nonTsFiles": {
///   "**/*.module.css": { "default": "package", "*": "package" }
/// }
/// ```
///
/// Entries keep their written order (`serde_json`'s `preserve_order` feature);
/// when several globs match a file, the first entry that assigns the imported
/// name — directly or via `"*"` — wins, so more specific patterns belong first.
/// An export no entry assigns falls back to `defaultImportability`, exactly like
/// a TS export with no JSDoc access tag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NonTsFilesOption {
    pub entries: Vec<NonTsFilesEntry>,
}

/// One `nonTsFiles` entry: a glob over project-relative resolved paths, plus the
/// export-name -> access mapping applied to files it matches.
#[derive(Debug, Clone, PartialEq)]
pub struct NonTsFilesEntry {
    pub pattern: String,
    /// Export name (or `"*"` for any non-default export) -> access level.
    pub exports: HashMap<String, Importability>,
}

impl<'de> Deserialize<'de> for NonTsFilesOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialized through a MapAccess visitor (rather than a derived map
        // field) so entries stay in the order the deserializer yields them —
        // which, with `preserve_order`, is the order they were written in.
        struct EntriesVisitor;

        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = NonTsFilesOption;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map from glob pattern to an export-name/access-level map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::new();
                while let Some((pattern, exports)) =
                    map.next_entry::<String, HashMap<String, Importability>>()?
                {
                    entries.push(NonTsFilesEntry { pattern, exports });
                }
                Ok(NonTsFilesOption { entries })
            }
        }

        deserializer.deserialize_map(EntriesVisitor)
    }
}

/// Options for the `package-access` rule (spec §4). Deserializes the exact
/// camelCase option names the reference plugin accepts: `indexLoophole`,
/// `filenameLoophole`, `defaultImportability`, `treatSelfReferenceAs`,
/// `excludeSourcePatterns`, `packageDirectory` — plus ImportLint's own
/// `nonTsFiles` (the reference plugin has no non-TS support). `deny_unknown_fields` is typo
/// protection for the config file (M5): a misspelled option name is a hard load
/// error rather than a silently ignored no-op. This is also what makes
/// `config::PackageAccessRuleConfig`'s `#[serde(flatten)]` field reject unknown
/// keys, since serde disallows `deny_unknown_fields` on a struct that itself has a
/// flatten field — the flattened type is where that check has to live instead.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct PackageAccessRuleOptions {
    pub index_loophole: bool,
    pub filename_loophole: bool,
    pub default_importability: Importability,
    pub treat_self_reference_as: SelfRefOpt,
    pub exclude_source_patterns: Vec<String>,
    pub package_directory: Option<Vec<String>>,
    pub non_ts_files: NonTsFilesOption,
}

impl Default for PackageAccessRuleOptions {
    fn default() -> Self {
        Self {
            index_loophole: true,
            filename_loophole: false,
            default_importability: Importability::Public,
            treat_self_reference_as: SelfRefOpt::External,
            exclude_source_patterns: Vec::new(),
            package_directory: None,
            non_ts_files: NonTsFilesOption::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_documented_defaults() {
        let opts = PackageAccessRuleOptions::default();
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
        let opts: PackageAccessRuleOptions = serde_json::from_value(json).unwrap();
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
        let opts: PackageAccessRuleOptions = serde_json::from_value(json).unwrap();
        assert!(opts.index_loophole);
        assert_eq!(opts.default_importability, Importability::Private);
        assert!(opts.non_ts_files.entries.is_empty());
    }

    #[test]
    fn non_ts_files_deserializes_entries_in_written_order() {
        // Written order is semantic (first matching entry wins), so it must
        // survive deserialization — this is what serde_json's `preserve_order`
        // feature is enabled for.
        let json = serde_json::json!({
            "nonTsFiles": {
                "**/global.css": { "*": "public" },
                "**/*.module.css": { "default": "package", "*": "private" }
            }
        });
        let opts: PackageAccessRuleOptions = serde_json::from_value(json).unwrap();
        let entries = &opts.non_ts_files.entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].pattern, "**/global.css");
        assert_eq!(
            entries[0].exports,
            HashMap::from([("*".to_string(), Importability::Public)])
        );
        assert_eq!(entries[1].pattern, "**/*.module.css");
        assert_eq!(
            entries[1].exports.get("default"),
            Some(&Importability::Package)
        );
        assert_eq!(entries[1].exports.get("*"), Some(&Importability::Private));
    }

    #[test]
    fn non_ts_files_rejects_an_invalid_access_level() {
        let json = serde_json::json!({ "nonTsFiles": { "**/*.css": { "*": "packge" } } });
        assert!(serde_json::from_value::<PackageAccessRuleOptions>(json).is_err());
    }

    #[test]
    fn non_ts_files_rejects_a_non_map_value() {
        let json = serde_json::json!({ "nonTsFiles": ["**/*.css"] });
        assert!(serde_json::from_value::<PackageAccessRuleOptions>(json).is_err());
    }
}
