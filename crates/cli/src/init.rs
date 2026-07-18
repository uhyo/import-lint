//! `import-lint init` (M9, `docs/PLAN-init.md`): scaffold a fully commented
//! `.importlintrc.jsonc` into the current directory. Scaffold-time only —
//! `crates/core` knows nothing about the template, and the generated file
//! carries no reference back to it (D-I2).
//!
//! There is exactly one template: the `*.package` naming convention with
//! `defaultImportability: "package"` (formerly the `standard` preset). Its
//! `packageDirectory` fallback — files outside every `*.package` directory all
//! share one project-root package — makes it suit gradual adoption on an
//! existing codebase as well as new projects, which is why the earlier
//! `--preset` selection (`gradual`, `monorepo`) was retired: alternative
//! setups are a config edit away and documented in `docs/guides/adoption.md`.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The scaffold-time template (D-I4): a complete, fully commented
/// `.importlintrc.jsonc`, adapted from the README's config example. A static
/// string, not a serialization — comments can't come out of `serde_json`.
pub const TEMPLATE: &str = r#"// .importlintrc.jsonc
//
// The `*.package` naming convention. Name any directory that should be an
// encapsulation boundary "foo.package" (e.g. "src/auth.package/",
// "src/billing.package/"). Everything inside a `*.package` directory, at any
// depth, imports freely from everything else inside it; nothing outside can
// import from it unless the export is tagged `@public`, or re-exported
// (unannotated) from the boundary's own index.ts — the index loophole below
// promotes those exports to the parent's package, and since a bare re-export
// resets to `defaultImportability`, wider exposure is always a deliberate,
// visible one-level-at-a-time edit.
//
// Directories you haven't renamed are unaffected — files outside every
// boundary all share one project-root package and import freely from each
// other — so this config works for gradual adoption on an existing codebase
// as well as for new projects: adopt boundaries one directory rename at a
// time.
{
  // Roots to walk for lint targets, relative to the project root.
  "include": ["."],

  // Extra glob patterns to skip, on top of .gitignore. Relative to the project root.
  "exclude": [],

  // Path to tsconfig.json (for resolver `paths`/`baseUrl`), relative to the
  // project root. Defaults to "<project root>/tsconfig.json" if it exists.
  // "tsconfig": "./tsconfig.json",

  "rules": {
    "package-access": {
      // "error" | "warn" | "off". An `off` rule is never checked.
      "severity": "error",

      // Below: identical options, names, and defaults to
      // eslint-plugin-import-access's `import-access/jsdoc` rule.

      // Treat a file named "index.{js,ts,jsx,tsx,mjs,cjs,...}" as if its parent
      // directory were the exporting file, for package-boundary purposes. This is
      // the escape valve for the `*.package` convention: a bare re-export from a
      // boundary's index.ts promotes that export to the parent's package.
      "indexLoophole": true,

      // Treat "foo/bar.ts" as in-package with "foo.ts" (one directory level,
      // matching the importer's own filename stem). Turn on for the
      // companion-file pattern: a "sub.ts" file living next to the "sub/"
      // directory it belongs to.
      // "filenameLoophole": true,
      "filenameLoophole": false,

      // Access level assumed for an export with no recognized JSDoc access tag.
      // "public" | "package" | "private". "package" makes every `*.package`
      // boundary (see packageDirectory below) an encapsulation boundary by
      // default, with no JSDoc tag required. Files outside every boundary all
      // share one project-root package, so they import freely from each other
      // — adopt boundaries one directory rename at a time.
      "defaultImportability": "package",

      // How a bare specifier matching the importer's own package name is
      // classified. "external" (never checked) | "internal" (checked normally).
      "treatSelfReferenceAs": "external",

      // Glob patterns (matched against the exporting file's project-relative
      // path) that are never checked, regardless of access level.
      "excludeSourcePatterns": [],

      // Glob patterns identifying "package" directories (matched against both
      // basename and project-relative path). A file with no matching ancestor
      // belongs to a single project-root package. A `!`-prefixed pattern
      // excludes a directory that would otherwise match. Below: the `*.package`
      // naming convention — a directory is a boundary because of its name, not
      // its location, so this never needs updating as the project grows.
      //
      // Alternative conventions (uncomment one instead, or drop the option
      // entirely to make every directory its own package):
      // "packageDirectory": ["**", "!**/*.internal"],  // inverse naming: every
      //   directory is a boundary except ones opting out with an ".internal" name
      // "packageDirectory": ["packages/*"],  // fixed-location: boundaries live
      //   under one top-level directory instead of being named by suffix
      "packageDirectory": ["**/*.package"],
    }
  }
}
"#;

/// Everything that can go wrong running `init`: an existing config without
/// `--force`, or an I/O failure writing the generated file.
#[derive(Debug)]
pub enum InitError {
    /// A `.importlintrc.jsonc`/`.importlintrc.json` already exists at this path
    /// and `--force` wasn't given (D-I6).
    ConfigExists(PathBuf),
    /// Writing the generated config file failed.
    Write { path: PathBuf, source: io::Error },
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::ConfigExists(path) => write!(
                f,
                "{} already exists (use --force to overwrite)",
                path.display()
            ),
            InitError::Write { path, source } => {
                write!(f, "failed to write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for InitError {}

/// Scaffold `.importlintrc.jsonc` into `cwd`, which thereby becomes the project
/// root (D-I1).
///
/// D-I6 guards: refuses if `.importlintrc.jsonc` or `.importlintrc.json`
/// already exists in `cwd`, unless `force`. All human output (notes, the
/// success message) goes to stderr; nothing is ever written to stdout (D-I7).
pub fn run_init(cwd: &Path, force: bool) -> Result<(), InitError> {
    let jsonc_path = cwd.join(".importlintrc.jsonc");
    let json_path = cwd.join(".importlintrc.json");
    let jsonc_exists = jsonc_path.is_file();
    let json_exists = json_path.is_file();

    if !force {
        if jsonc_exists {
            return Err(InitError::ConfigExists(jsonc_path));
        }
        if json_exists {
            return Err(InitError::ConfigExists(json_path));
        }
    }

    fs::write(&jsonc_path, TEMPLATE).map_err(|err| InitError::Write {
        path: jsonc_path.clone(),
        source: err,
    })?;

    let mut stderr = io::stderr();
    if force && json_exists {
        let _ = writeln!(
            stderr,
            "note: {} now shadows {} (a .jsonc config always wins over .json in the same directory)",
            jsonc_path.display(),
            json_path.display()
        );
    }
    // Neither file existed in `cwd` itself, so this isn't an overwrite — but an
    // ancestor directory's config, if any, just lost the project root to this
    // new file (D-I6).
    if !jsonc_exists
        && !json_exists
        && let Some(parent) = cwd.parent()
        && let Some(ancestor_config) = import_lint::find_config(parent)
    {
        let _ = writeln!(
            stderr,
            "note: {} takes over the project root for this subtree (was: {})",
            jsonc_path.display(),
            ancestor_config.display()
        );
    }
    let _ = writeln!(stderr, "Wrote {}", jsonc_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use import_lint::rule::Importability;
    use tempfile::TempDir;

    #[test]
    fn template_round_trips_and_has_expected_options() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".importlintrc.jsonc");
        fs::write(&path, TEMPLATE).unwrap();
        let config = import_lint::LintConfig::load(&path).expect("should parse");
        assert_eq!(
            config.rules.package_access.options.default_importability,
            Importability::Package
        );
        assert_eq!(
            config.rules.package_access.options.package_directory,
            Some(vec!["**/*.package".to_string()])
        );
        assert_eq!(config.include, vec!["."]);
        assert!(config.exclude.is_empty());
    }
}
