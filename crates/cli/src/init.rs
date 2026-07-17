//! `import-lint init` (M9, `docs/PLAN.md`): scaffold a fully commented
//! `.importlintrc.jsonc` into the current directory, from one of three curated
//! presets. Scaffold-time only — `crates/core` knows nothing about presets, and
//! the generated file carries no reference back to the one that produced it
//! (D-I2).

use std::fmt;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use clap::ValueEnum;

/// A scaffold-time config template, selected via `--preset <name>` or the
/// interactive picker (D-I2). The `jsdoc` rule's two axes that actually
/// distinguish real-world setups — `defaultImportability` and
/// `packageDirectory` — are what vary between presets; everything else in the
/// generated file is the same fully commented skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Preset {
    /// The `*.package` naming convention: directories named `foo.package` are
    /// encapsulation boundaries; exports are package-scoped unless `@public`
    /// (recommended for new projects).
    Standard,
    /// Annotation-driven: exports stay public until tagged `@package`/`@private`
    /// (for adopting on an existing codebase).
    Gradual,
    /// Boundaries at `packages/*`: no relative reach-ins across workspace
    /// packages.
    Monorepo,
}

impl Preset {
    /// The kebab-case name clap parses `--preset` values as (`standard`,
    /// `gradual`, `monorepo`) — reused by the interactive menu and `init`'s
    /// success message so there's exactly one name per preset.
    fn id(self) -> &'static str {
        match self {
            Preset::Standard => "standard",
            Preset::Gradual => "gradual",
            Preset::Monorepo => "monorepo",
        }
    }

    /// The one-line description shown by both `--help` and the interactive menu
    /// (D-I5) — sourced from the doc comment above via clap's own
    /// `ValueEnum`/`PossibleValue` machinery, so there's exactly one copy of the
    /// text.
    fn description(self) -> String {
        self.to_possible_value()
            .and_then(|value| value.get_help().map(ToString::to_string))
            .expect("every Preset variant has a doc comment, which clap turns into help text")
    }
}

/// This preset's scaffold-time template (D-I4): a complete, fully commented
/// `.importlintrc.jsonc`, adapted from the README's config example. A static
/// string, not a serialization — comments can't come out of `serde_json`.
pub fn template(preset: Preset) -> &'static str {
    match preset {
        Preset::Standard => STANDARD_TEMPLATE,
        Preset::Gradual => GRADUAL_TEMPLATE,
        Preset::Monorepo => MONOREPO_TEMPLATE,
    }
}

const GRADUAL_TEMPLATE: &str = r#"// .importlintrc.jsonc
//
// Preset: gradual — incremental adoption on an existing codebase. Nothing is
// restricted until you tag an export `@package` or `@private`; every rule
// option below is at its default (this file is, deliberately, close to the
// README's own config example).
{
  // Roots to walk for lint targets, relative to the project root.
  "include": ["."],

  // Extra glob patterns to skip, on top of .gitignore. Relative to the project root.
  "exclude": [],

  // Path to tsconfig.json (for resolver `paths`/`baseUrl`), relative to the
  // project root. Defaults to "<project root>/tsconfig.json" if it exists.
  // "tsconfig": "./tsconfig.json",

  "rules": {
    "jsdoc": {
      // "error" | "warn" | "off". An `off` rule is never checked.
      "severity": "error",

      // Below: identical options, names, and defaults to
      // eslint-plugin-import-access's `import-access/jsdoc` rule.

      // Treat a file named "index.{js,ts,jsx,tsx,mjs,cjs,...}" as if its parent
      // directory were the exporting file, for package-boundary purposes.
      "indexLoophole": true,

      // Treat "foo/bar.ts" as in-package with "foo.ts" (one directory level,
      // matching the importer's own filename stem).
      "filenameLoophole": false,

      // Access level assumed for an export with no recognized JSDoc access tag.
      // "public" | "package" | "private"
      "defaultImportability": "public",

      // How a bare specifier matching the importer's own package name is
      // classified. "external" (never checked) | "internal" (checked normally).
      "treatSelfReferenceAs": "external",

      // Glob patterns (matched against the exporting file's project-relative
      // path) that are never checked, regardless of access level.
      "excludeSourcePatterns": [],

      // Glob patterns identifying "package" directories (matched against both
      // basename and project-relative path). Unset: a file's own containing
      // directory is its package. A `!`-prefixed pattern excludes a directory
      // that would otherwise match.
      // "packageDirectory": ["packages/*"],
    }
  }
}
"#;

const STANDARD_TEMPLATE: &str = r#"// .importlintrc.jsonc
//
// Preset: standard — the `*.package` naming convention (recommended for new
// projects). Name any directory that should be an encapsulation boundary
// "foo.package" (e.g. "src/auth.package/", "src/billing.package/"). Everything
// inside a `*.package` directory, at any depth, imports freely from everything
// else inside it; nothing outside can import from it unless the export is
// tagged `@public`, or re-exported (unannotated) from the boundary's own
// index.ts — the index loophole below promotes those exports to the parent's
// package, and since a bare re-export resets to `defaultImportability`,
// wider exposure is always a deliberate, visible one-level-at-a-time edit.
{
  // Roots to walk for lint targets, relative to the project root.
  "include": ["."],

  // Extra glob patterns to skip, on top of .gitignore. Relative to the project root.
  "exclude": [],

  // Path to tsconfig.json (for resolver `paths`/`baseUrl`), relative to the
  // project root. Defaults to "<project root>/tsconfig.json" if it exists.
  // "tsconfig": "./tsconfig.json",

  "rules": {
    "jsdoc": {
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
      // boundary (see packageDirectory below) — and, outside of one, every
      // plain directory — an encapsulation boundary by default, with no JSDoc
      // tag required.
      "defaultImportability": "package",

      // How a bare specifier matching the importer's own package name is
      // classified. "external" (never checked) | "internal" (checked normally).
      "treatSelfReferenceAs": "external",

      // Glob patterns (matched against the exporting file's project-relative
      // path) that are never checked, regardless of access level.
      "excludeSourcePatterns": [],

      // Glob patterns identifying "package" directories (matched against both
      // basename and project-relative path). A `!`-prefixed pattern excludes a
      // directory that would otherwise match. Below: the `*.package` naming
      // convention — a directory is a boundary because of its name, not its
      // location, so this never needs updating as the project grows.
      //
      // Alternative conventions (uncomment one instead, or drop the option
      // entirely to fall back to per-directory scoping outside any boundary):
      // "packageDirectory": ["**", "!**/*.internal"],  // inverse naming: every
      //   directory is a boundary except ones opting out with an ".internal" name
      // "packageDirectory": ["src/packages/*"],  // fixed-location: boundaries
      //   live under one top-level directory instead of being named by suffix
      "packageDirectory": ["**/*.package"],
    }
  }
}
"#;

const MONOREPO_TEMPLATE: &str = r#"// .importlintrc.jsonc
//
// Preset: monorepo — boundaries at the workspace-package level. A deep relative
// reach-in across sibling packages ("../../other-pkg/src/...") becomes an error
// unless the export is `@public`; a name-based import of a sibling workspace
// package (resolved through node_modules) stays exempt as external, same as any
// other external dependency. Inside one workspace package, imports are
// unrestricted.
{
  // Roots to walk for lint targets, relative to the project root.
  "include": ["."],

  // Extra glob patterns to skip, on top of .gitignore. Relative to the project root.
  "exclude": [],

  // Path to tsconfig.json (for resolver `paths`/`baseUrl`), relative to the
  // project root. Defaults to "<project root>/tsconfig.json" if it exists.
  // "tsconfig": "./tsconfig.json",

  "rules": {
    "jsdoc": {
      // "error" | "warn" | "off". An `off` rule is never checked.
      "severity": "error",

      // Below: identical options, names, and defaults to
      // eslint-plugin-import-access's `import-access/jsdoc` rule.

      // Treat a file named "index.{js,ts,jsx,tsx,mjs,cjs,...}" as if its parent
      // directory were the exporting file, for package-boundary purposes.
      "indexLoophole": true,

      // Treat "foo/bar.ts" as in-package with "foo.ts" (one directory level,
      // matching the importer's own filename stem).
      // "filenameLoophole": true,
      "filenameLoophole": false,

      // Access level assumed for an export with no recognized JSDoc access tag.
      // "public" | "package" | "private". "package" makes each workspace
      // package (see packageDirectory below) an encapsulation boundary by
      // default.
      "defaultImportability": "package",

      // How a bare specifier matching the importer's own package name is
      // classified. "external" (never checked) | "internal" (checked normally).
      // Left at "external": a sibling workspace package imported by name (not
      // by relative path) resolves through node_modules and is exempt, same as
      // any other external dependency.
      "treatSelfReferenceAs": "external",

      // Glob patterns (matched against the exporting file's project-relative
      // path) that are never checked, regardless of access level.
      "excludeSourcePatterns": [],

      // Glob patterns identifying "package" directories (matched against both
      // basename and project-relative path). Adjust to match your workspace
      // layout — e.g. add "apps/*" if you also have an apps/ directory of
      // workspace packages.
      "packageDirectory": ["packages/*"],
    }
  }
}
"#;

/// Everything that can go wrong running `init`: an existing config without
/// `--force`, an I/O failure reading the interactive preset choice, or an I/O
/// failure writing the generated file.
#[derive(Debug)]
pub enum InitError {
    /// A `.importlintrc.jsonc`/`.importlintrc.json` already exists at this path
    /// and `--force` wasn't given (D-I6).
    ConfigExists(PathBuf),
    /// Reading the interactive preset choice failed (EOF or an I/O error).
    Prompt(io::Error),
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
            InitError::Prompt(err) => write!(f, "failed to read preset selection: {err}"),
            InitError::Write { path, source } => {
                write!(f, "failed to write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for InitError {}

/// Scaffold `.importlintrc.jsonc` into `cwd`, which thereby becomes the project
/// root (D-I1). `preset: None` means run the interactive picker against real
/// stdin/stderr — the caller (`main.rs`) is responsible for only passing `None`
/// when stdin and stderr are both TTYs (D-I5); this function has no TTY logic of
/// its own.
///
/// D-I6 guards: refuses — without ever prompting — if `.importlintrc.jsonc` or
/// `.importlintrc.json` already exists in `cwd`, unless `force`. All human output
/// (the interactive prompt, notes, the success message) goes to stderr; nothing
/// is ever written to stdout (D-I7).
pub fn run_init(cwd: &Path, preset: Option<Preset>, force: bool) -> Result<(), InitError> {
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

    let preset = match preset {
        Some(preset) => preset,
        None => {
            let stdin = io::stdin();
            let mut stderr = io::stderr();
            choose_preset(stdin.lock(), &mut stderr).map_err(InitError::Prompt)?
        }
    };

    fs::write(&jsonc_path, template(preset)).map_err(|err| InitError::Write {
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
    let _ = writeln!(
        stderr,
        "Wrote {} (preset: {})",
        jsonc_path.display(),
        preset.id()
    );

    Ok(())
}

/// The interactive preset picker (D-I5): a hand-rolled numbered menu, the menu
/// and each prompt line written to `out`, one line read from `input` per
/// attempt. Empty input picks `standard`; invalid input re-prompts; EOF is an
/// error. A pure function over a reader/writer — no TTY logic here, which is
/// what makes it unit-testable with a `Cursor` and swappable for a fancier
/// picker later (R-I3) without touching the caller.
pub fn choose_preset(mut input: impl BufRead, mut out: impl Write) -> io::Result<Preset> {
    let presets = Preset::value_variants();

    writeln!(out, "Choose a preset:")?;
    for (i, preset) in presets.iter().enumerate() {
        writeln!(
            out,
            "  {}) {:<9}— {}",
            i + 1,
            preset.id(),
            preset.description()
        )?;
    }

    loop {
        write!(out, "Preset [1]: ")?;
        out.flush()?;

        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "no input while choosing a preset (pass --preset <name> for non-interactive use)",
            ));
        }

        let choice = line.trim();
        if choice.is_empty() {
            return Ok(Preset::Standard);
        }
        if let Some(preset) = choice
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| presets.get(i))
        {
            return Ok(*preset);
        }
        writeln!(out, "Not a valid choice: {choice:?}")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use import_lint::rule::Importability;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn write_template(preset: Preset) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".importlintrc.jsonc");
        fs::write(&path, template(preset)).unwrap();
        (dir, path)
    }

    #[test]
    fn standard_template_round_trips_and_has_distinguishing_options() {
        let (_dir, path) = write_template(Preset::Standard);
        let config = import_lint::LintConfig::load(&path).expect("should parse");
        assert_eq!(
            config.rules.jsdoc.options.default_importability,
            Importability::Package
        );
        assert_eq!(
            config.rules.jsdoc.options.package_directory,
            Some(vec!["**/*.package".to_string()])
        );
    }

    #[test]
    fn gradual_template_round_trips_at_all_defaults() {
        let (_dir, path) = write_template(Preset::Gradual);
        let config = import_lint::LintConfig::load(&path).expect("should parse");
        let defaults = import_lint::JsdocRuleOptions::default();
        assert_eq!(
            config.rules.jsdoc.options.default_importability,
            defaults.default_importability
        );
        assert_eq!(
            config.rules.jsdoc.options.index_loophole,
            defaults.index_loophole
        );
        assert_eq!(
            config.rules.jsdoc.options.filename_loophole,
            defaults.filename_loophole
        );
        assert_eq!(
            config.rules.jsdoc.options.package_directory,
            defaults.package_directory
        );
        assert_eq!(config.include, vec!["."]);
        assert!(config.exclude.is_empty());
    }

    #[test]
    fn monorepo_template_round_trips_and_has_distinguishing_options() {
        let (_dir, path) = write_template(Preset::Monorepo);
        let config = import_lint::LintConfig::load(&path).expect("should parse");
        assert_eq!(
            config.rules.jsdoc.options.default_importability,
            Importability::Package
        );
        assert_eq!(
            config.rules.jsdoc.options.package_directory,
            Some(vec!["packages/*".to_string()])
        );
    }

    #[test]
    fn choose_preset_picks_by_number() {
        for (input, expected) in [
            ("1\n", Preset::Standard),
            ("2\n", Preset::Gradual),
            ("3\n", Preset::Monorepo),
        ] {
            let mut out = Vec::new();
            let preset = choose_preset(Cursor::new(input), &mut out).expect("should succeed");
            assert_eq!(preset, expected, "input {input:?}");
        }
    }

    #[test]
    fn choose_preset_empty_line_defaults_to_standard() {
        let mut out = Vec::new();
        let preset = choose_preset(Cursor::new("\n"), &mut out).expect("should succeed");
        assert_eq!(preset, Preset::Standard);
    }

    #[test]
    fn choose_preset_reprompts_on_garbage_then_succeeds() {
        let mut out = Vec::new();
        let preset = choose_preset(Cursor::new("nope\n4\n2\n"), &mut out).expect("should succeed");
        assert_eq!(preset, Preset::Gradual);
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered.matches("Preset [1]:").count(), 3);
        assert!(rendered.contains("Not a valid choice"));
    }

    #[test]
    fn choose_preset_eof_errors() {
        let mut out = Vec::new();
        let err = choose_preset(Cursor::new(""), &mut out).expect_err("should error on EOF");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
