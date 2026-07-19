//! `import-lint docs` / `import-lint explain`: built-in documentation, so the
//! text always matches the installed binary's flags and semantics.
//!
//! The content is canonical here rather than `include_str!`-ing
//! `docs/guides/*`: the published crate cannot package files outside the crate
//! root, and these texts are deliberately condensed for terminal (and AI-agent)
//! consumption — the guides stay the long-form reference and every text links
//! to them.

/// `import-lint docs` with no topic: the topic index (stdout, exit 0).
pub const TOPICS_INDEX: &str = "\
Built-in documentation topics (import-lint docs <topic>):

  concepts   The mental model: packages, importability, loopholes, re-exports
  config     Config file reference: discovery, all options and their defaults
  fixing     How to fix an import-access violation

`import-lint explain <message-id>` explains an individual diagnostic.
Full guides: https://github.com/uhyo/import-lint/tree/master/docs/guides
";

/// `import-lint explain` with no id: the message-id index (stdout, exit 0).
pub const EXPLAIN_INDEX: &str = "\
Diagnostic message ids (import-lint explain <message-id>):

  package            Cannot import a package-private export
  package:reexport   Cannot re-export a package-private export
  private            Cannot import a private export
  private:reexport   Cannot re-export a private export
  unresolved         Unresolved import specifier (with --report-unresolved)

`import-lint --format json` includes each diagnostic's id in its `messageId`
field; the default pretty format shows the message text only.
Topic guides: `import-lint docs`
";

/// Comma-separated topic names, for the unknown-topic error message.
pub const TOPIC_NAMES: &str = "concepts, config, fixing";

/// Comma-separated message ids, for the unknown-id error message.
pub const EXPLAIN_IDS: &str = "package, package:reexport, private, private:reexport, unresolved";

pub fn topic(name: &str) -> Option<&'static str> {
    match name {
        "concepts" => Some(CONCEPTS),
        "config" => Some(CONFIG),
        "fixing" => Some(FIXING),
        _ => None,
    }
}

pub fn explanation(id: &str) -> Option<&'static str> {
    match id {
        "package" => Some(EXPLAIN_PACKAGE),
        "package:reexport" => Some(EXPLAIN_PACKAGE_REEXPORT),
        "private" => Some(EXPLAIN_PRIVATE),
        "private:reexport" => Some(EXPLAIN_PRIVATE_REEXPORT),
        "unresolved" => Some(EXPLAIN_UNRESOLVED),
        _ => None,
    }
}

const CONCEPTS: &str = r#"ImportLint concepts

ImportLint enforces directory-level encapsulation in TypeScript/JavaScript.
A directory is a "package" (unrelated to npm packages); its exports are
importable only from files inside it until an export is explicitly opened up.

Importability
  Every export has one of three access levels:
    public    importable from anywhere
    package   importable only from files in the same package
    private   importable from nowhere (not even the same package)
  Declared with a JSDoc tag directly above the export (case-sensitive):
      /** @package */
      export const token = "...";
  `/** @access package */` is an equivalent spelling. Line comments
  (`// @package`) are not recognized. An export with no recognized tag falls
  back to the config's `defaultImportability` (built-in default: "public";
  configs scaffolded by `import-lint init` set "package").

Packages and boundaries
  By default every directory is its own package. Files in a child directory
  may import from ancestor packages; the reverse is a violation. If the
  config sets `packageDirectory` (glob patterns, e.g. "**/*.package"), only
  matching directories are boundaries, and a file with no matching ancestor
  belongs to a single project-root package.

Index loophole (`indexLoophole`, default: on)
  A bare re-export in a package's index file, e.g.
      export { issueToken } from "./token";
  promotes that export one level out, to the parent's package. This is the
  idiomatic way to expose a package's API without making it fully public.

Filename loophole (`filenameLoophole`, default: off)
  Treats "foo/bar.ts" as in-package with the companion file "foo.ts" (one
  directory level).

One-hop re-export semantics
  A re-export statement's own JSDoc tag governs visibility for whoever
  imports through it; ImportLint never looks a second hop further. A bare
  (untagged) re-export RESETS importability to `defaultImportability` — even
  if the original export was tagged @public. The re-export statement itself
  is also checked against the file it re-exports from.

Internal vs. external
  Only imports resolving to files inside the project are checked; npm
  dependencies and Node builtins are never flagged.

Full guide:
https://github.com/uhyo/import-lint/blob/master/docs/guides/concepts.md
"#;

const CONFIG: &str = r#"Config file reference

File and discovery
  `.importlintrc.jsonc` (JSON with comments), or `.importlintrc.json` as a
  fallback when no .jsonc exists in the same directory. Discovered by walking
  up from the current directory, unless `--config <path>` names one
  explicitly. The directory containing the config file becomes the project
  root; `include`, `exclude`, and `tsconfig` are resolved relative to it.
  Without a config file, defaults apply and the current directory is the
  project root. Unknown keys anywhere in the config are a hard load error
  (exit code 2), never silently ignored. `import-lint init` scaffolds the
  recommended setup.

Top-level keys
  "include": ["."]     Roots to walk for lint targets.
  "exclude": []        Extra glob patterns to skip, on top of .gitignore.
  "tsconfig": <path>   tsconfig.json for resolver paths/baseUrl (default:
                       "<project root>/tsconfig.json" if it exists).
  "rules": { "package-access": { ... } }

"package-access" rule options (defaults shown)
  "severity": "error"
      "error" | "warn" | "off". Warnings are shown but never affect the
      exit code.
  "indexLoophole": true
      A bare re-export in a boundary's index file promotes the export to
      the parent's package.
  "filenameLoophole": false
      "foo.ts" is in-package with the contents of the "foo/" directory
      next to it (one level).
  "defaultImportability": "public"
      Access level of exports with no JSDoc tag: "public" | "package" |
      "private". `import-lint init` scaffolds "package".
  "treatSelfReferenceAs": "external"
      Imports of the project's own package.json name: "external" (never
      checked) | "internal" (checked normally).
  "excludeSourcePatterns": []
      Glob patterns (matched against the exporting file's project-relative
      path) that are never checked.
  "packageDirectory": (unset)
      Glob patterns naming boundary directories (matched against both the
      basename and the project-relative path). Unset: every directory is
      its own package. A "!"-prefixed pattern excludes an otherwise-
      matching directory.

Full reference:
https://github.com/uhyo/import-lint/blob/master/README.md#config-file
"#;

const FIXING: &str = r#"Fixing an import-access violation

First locate the exporting file and its package boundary: the nearest
ancestor directory matching the config's `packageDirectory` patterns, or the
exporting file's own directory if `packageDirectory` is unset. Then pick the
first fix that fits:

  1. Move the importing file into the package. If it conceptually belongs
     there, no visibility change is needed at all.

  2. Re-export through the package's index file (uses the index loophole,
     on by default). Add a bare re-export to the boundary's index.ts:
         export { issueToken } from "./token";
     This exposes the export one level out — to the parent's package, not
     the whole project. To widen visibility further, tag the re-export
     line itself:
         /** @public */
         export { issueToken } from "./token";

  3. Tag the original export `/** @public */`, making it importable from
     anywhere. Rarely the right choice — it gives up the boundary for that
     export; prefer 1 or 2.

During gradual adoption, config-level changes can also be appropriate:
adjusting `packageDirectory` patterns, `defaultImportability`, or
`excludeSourcePatterns`. Do not reach for those (or `"severity": "off"`)
just to silence one diagnostic — they weaken checking project-wide.

After editing, rerun `import-lint` to confirm the diagnostic is gone and no
new ones appeared. `import-lint explain <message-id>` explains an individual
diagnostic (`--format json` reports each diagnostic's `messageId`).

Walkthrough:
https://github.com/uhyo/import-lint/blob/master/docs/guides/tutorial.md
"#;

const EXPLAIN_PACKAGE: &str = r#"package — "Cannot import a package-private export '...'"

The import resolves to a file inside your project, and that export's
importability is `package`: importable only from files within the same
package (the exporting file's boundary directory, or anywhere below it).
The importing file is outside that package.

The importability came from a `/** @package */` JSDoc tag directly above the
export, or — with no tag — from the config's `defaultImportability`.

Fixes, in order of preference:
  1. Move the importing file inside the package, if it conceptually belongs
     there. No visibility change needed.
  2. Bare re-export from the package's index file (index loophole, on by
     default):
         export { theName } from "./the-module";
     This promotes it one level out, to the parent's package. To widen
     further, tag the re-export line itself (e.g. `/** @public */`).
  3. Tag the original export `/** @public */` to allow importing from
     anywhere. Rarely the right choice — prefer 1 or 2.

Do not silence it by setting `"severity"` to "off"/"warn", adding
`excludeSourcePatterns`, or loosening `defaultImportability` — those weaken
checking project-wide. See `import-lint docs fixing`.
"#;

const EXPLAIN_PACKAGE_REEXPORT: &str = r#"package:reexport — "Cannot re-export a package-private export '...'"

A re-export statement (e.g. `export { x } from "./mod"` or
`export * from "./mod"`) is itself checked like an import against the file
it re-exports from. Here, that file's export is package-private and the
re-exporting file is outside its package.

Fixes, in order of preference:
  1. Re-export from inside instead: a bare re-export in the *owning*
     package's own index file (index loophole) promotes the export one
     level out; chain one level at a time rather than reaching across the
     boundary from outside.
  2. Move the re-exporting file into the owning package.
  3. Tag the original export `/** @public */`. Rarely the right choice.

See `import-lint docs fixing` and `import-lint explain package`.
"#;

const EXPLAIN_PRIVATE: &str = r#"private — "Cannot import a private export '...'"

The export's importability is `private`: importable from nowhere, not even
from files in the same package. That comes from a `/** @private */` JSDoc
tag directly above the export, or — with no tag — from
`defaultImportability: "private"` in the config.

Fixes:
  1. If the export should be usable within its package, change its tag to
     `/** @package */` (or remove the tag, when the config's
     `defaultImportability` allows more).
  2. If it should be importable one level out, give the package's index
     file a *tagged* re-export, e.g.:
         /** @package */
         export { theName } from "./the-module";
     Note: under `defaultImportability: "private"`, a bare re-export resets
     to private again — the tag on the re-export line is what opens it up.
  3. If the export exists only for the exporting file itself, stop
     importing it and inline or duplicate what you need.
"#;

const EXPLAIN_PRIVATE_REEXPORT: &str = r#"private:reexport — "Cannot re-export a private export '...'"

A re-export statement reaches into an export whose importability is
`private` — importable (and re-exportable) from nowhere.

Fixes: give the original export a wider access level if it is meant to be
shared — `/** @package */` at minimum — then follow the normal promotion
path (tagged re-exports from the package's index file, one level at a
time). See `import-lint explain private` and `import-lint docs fixing`.
"#;

const EXPLAIN_UNRESOLVED: &str = r#"unresolved — "Unresolved import specifier '...'"

Emitted only with `--report-unresolved`, always as a warning (never affects
the exit code). The resolver could not map this import specifier to a
project file or an external package, so ImportLint skipped checking it.
This is not an access violation — but an import that silently fails to
resolve is also never checked, so violations could hide behind it.

Common causes:
  - A typo in the specifier, or the target file is missing.
  - A tsconfig `paths`/`baseUrl` alias the resolver doesn't know about:
    point ImportLint at the right tsconfig via `--tsconfig <path>` or the
    config's `"tsconfig"` key (default: <project root>/tsconfig.json).
  - The specifier only resolves through bundler-specific magic ImportLint's
    resolver (oxc_resolver, Node-style) doesn't model.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use import_lint::MessageId;

    /// Every `MessageId` the rule engine can emit — plus the CLI-side
    /// `unresolved` — has an explanation, and stays listed in the indexes.
    #[test]
    fn every_message_id_has_an_explanation() {
        for id in [
            MessageId::Package,
            MessageId::PackageReexport,
            MessageId::Private,
            MessageId::PrivateReexport,
        ] {
            let text =
                explanation(id.as_str()).unwrap_or_else(|| panic!("no explanation for {:?}", id));
            assert!(
                text.starts_with(id.as_str()),
                "explanation for {:?} should lead with its id",
                id
            );
            assert!(EXPLAIN_INDEX.contains(id.as_str()));
            assert!(EXPLAIN_IDS.contains(id.as_str()));
        }
        assert!(explanation("unresolved").is_some());
    }

    #[test]
    fn every_topic_is_listed_in_the_index() {
        for name in ["concepts", "config", "fixing"] {
            assert!(topic(name).is_some());
            assert!(TOPICS_INDEX.contains(name));
            assert!(TOPIC_NAMES.contains(name));
        }
    }
}
