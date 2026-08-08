# ImportLint — Restricting Imports of Non-TS Files (`file-access` rule)

**Status: design proposal (not yet implemented).**

Today ImportLint can only restrict imports of things it can parse: TS/JS
modules, whose exports carry JSDoc access tags. Imports of anything else —
`*.module.css`, `.json`, `.svg`, `.png`, `.sql`, … — are invisible to the
linter. This plan adds the capability to restrict them, via a new
config-driven rule, **`file-access`**.

Motivating example: a CSS module is conventionally private to the component
it's colocated with. Nothing enforces that today — any file anywhere can
`import styles from "../components/Button/Button.module.css"` and silently
couple itself to another component's styling.

---

## 1. Investigation: how non-TS imports are treated today

Traced through the pipeline (and verified empirically on a fixture with
`import-lint graph`):

1. **Extraction** (`crates/cli/src/runner.rs`, `crates/core/src/extract/`):
   only files whose extension `oxc_span::SourceType::from_path` recognizes
   (`.ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs` + `.d.*`) are parsed. In the
   *importer* (a TS file), `import styles from "./a.module.css"` records the
   specifier plus a default-import checked entry; a side-effect
   `import "./a.css"` and a namespace `import * as ns` record **only the
   specifier** — no checked entry, hence no span usable for a diagnostic.
   Dynamic `import(...)` is not extracted at all.

2. **Resolution** (`crates/core/src/resolve/`): `ProjectResolver::resolve`
   goes through `oxc_resolver`'s `resolve_dts()`, the *TypeScript-declaration*
   entry point. It fails for targets that have no TS meaning: **both
   `./a.module.css` and `./data.json` come back `Err` → `Provenance::Unresolved`**,
   even when the file exists on disk. Wildcard ambient declarations
   (`declare module "*.module.css"` in a `global.d.ts`) do not change this:
   the ambient-module registry (D6) is exact-match only, and is consulted
   only for *bare* specifiers anyway.

3. **Check** (`crates/core/src/rule/mod.rs`): `Provenance::Unresolved` is
   skipped silently (D8). Net effect: **every non-TS import is
   unconditionally allowed**, and there is no configuration that can change
   that.

Two side observations worth fixing along the way:

- `--report-unresolved` flags every existing-and-fine `*.module.css` import
  as `Unresolved import specifier` — a false alarm that makes the flag
  noisy on any CSS-modules codebase.
- A latent stderr-noise path: if a non-parseable file ever *did* resolve as
  `Provenance::Internal`, the fixpoint loop would try to extract it and print
  `unrecognized file extension, skipping` (`runner.rs`). Not reachable today
  (resolution fails first), but the design below must not make it reachable.

Watch mode needs no structural work: `Create`/`Remove`/rename of *any* path
(css included) is already classified `Structural` (full re-run), and content
edits of unrecognized extensions are ignored — which stays correct, since a
restricted file's *content* never affects this rule.

## 2. Product decisions

| # | Decision | Rationale |
|---|---|---|
| D-F1 | **A new rule, `rules."file-access"`** — not new options on `package-access`. | `package-access` stays a 1:1 behavioral port of `eslint-plugin-import-access` (the migration story is "options map name-for-name"; adding options there breaks it). A sibling rule gets its own severity (e.g. `warn` while adopting), its own suppression-directive name, and its own rule id in output. The `rules` map was explicitly designed to admit future rules without a config-shape break (PLAN-v1.md §4). |
| D-F2 | **File-level semantics**: a restriction applies to the target file as a unit. *Every* syntactic reference counts — named/default/namespace/side-effect imports, `export ... from`, `export * from`. One diagnostic per referencing statement, spanning the module-specifier string literal. | Non-TS files have no exports to annotate, so per-export granularity is meaningless; and a restriction you can bypass with `import "./x.css"` or `import * as ns` is not a restriction. Requires a small extraction addition: record `(specifier, span-of-source-string)` per module-referencing statement (today only named/default/re-export entries carry spans). |
| D-F3 | **Config shape: an ordered `files` array of `{ pattern, importability }` entries; last matching entry wins.** `pattern` is a glob (string or array of strings) matched against the **resolved target file's project-relative path**, compiled with `literal_separator(true)` — same matching rules as `excludeSourcePatterns`. | An array is order-preserving by construction (a JSON object's key order is a serde implementation detail), and each entry is a natural extension point (a future `allowFrom`, custom message, …). Last-wins is the ESLint-overrides / gitignore convention: broad rule first, targeted exceptions after. Matching the *resolved path* (not the specifier) means tsconfig `paths` aliases, `../` chains, and specifier spelling differences all collapse to one pattern. |
| D-F4 | **`importability` reuses the established vocabulary**: `"public"` (no restriction — the implicit default for unmatched files), `"package"` (importable only from inside the target file's package), `"private"` (not importable from anywhere). | One mental model across the tool. `"package"` is the flagship: with no `packageDirectory` configured, a file's package is its own directory, so `"**/*.module.css": "package"` reads "a CSS module is importable from its own directory (and below)" — exactly the colocation convention. `"public"` exists as the override arm (e.g. re-allow `src/styles/global.css` after restricting `**/*.css`). `"private"` seals a file entirely (e.g. ban importing `.env`-ish or fixture files). |
| D-F5 | **Package geometry is shared with `package-access`**: `file-access` runs the same `is_in_package` check with the same `indexLoophole` / `filenameLoophole` / `packageDirectory` settings, read from `rules."package-access"` — including when that rule's severity is `off`. `file-access` defines no boundary options of its own. | One project has one package structure; two rules disagreeing about where boundaries lie would be incoherent, and duplicating `packageDirectory` invites exactly that drift. The loopholes compose naturally rather than needing asset-specific special cases: `indexLoophole` is inert for assets (`index.[cm]?[jt]sx?` can't match `index.module.css`), while `filenameLoophole` does something genuinely useful — `Button/styles.module.css` counts as in-package with `Button.tsx`. A per-rule boundary override is deliberately deferred until someone demonstrates a need (§6). |
| D-F6 | **Resolution: a fallback pass for specifiers `resolve_dts()` can't resolve.** When the dts resolver fails, retry with a plain (non-dts) `oxc_resolver` sharing the same tsconfig (`paths`/`baseUrl`), `symlinks: true`, and provenance classification — after stripping a bundler query/fragment suffix (`?url`, `?raw`, `#…`) from the specifier. Outcomes: `node_modules` → `External` (never restricted); a project file → a **new `Provenance::Asset(PathBuf)`**; still failing → `Unresolved` exactly as today. The fallback runs unconditionally, not only when `file-access` is configured. | The dts resolver is the correct primary (it's what makes TS-only packages resolvable) — but it structurally cannot see files TS assigns no types to (§1.2), so a second, cheap, on-failure-only pass is required. A distinct `Asset` variant (rather than reusing `Internal`) keeps the fixpoint loop from attempting extraction (no stderr noise, no wasted I/O) and keeps `package-access`'s `Internal`-only matching untouched. Running it unconditionally also fixes the `--report-unresolved` false alarms and makes `import-lint graph` output truthful — resolvable asset imports stop being lumped in with genuinely broken specifiers. |
| D-F7 | **Patterns may match parseable source files too.** A `files` entry whose glob matches a `.ts`/`.js` target (`Provenance::Internal`) restricts it identically, at file granularity. `package-access` continues to apply independently — one import line can violate both rules (distinct rule ids, separately suppressible). | Falls out of D-F3 for free (patterns match resolved paths; provenance kind doesn't matter), and it's genuinely useful: `{ "pattern": "**/generated/**", "importability": "private" }` seals a directory without annotating every export in it. Artificially excluding source files would be extra code to make the rule less capable. |
| D-F8 | **Diagnostics**: rule id `file-access`; message ids `package-file` ("Cannot import a package-private file '<specifier>'") and `private-file` ("Cannot import a private file '<specifier>'"). Suppression via the existing directives: `// import-lint-disable-next-line file-access`. `import-lint explain` gets entries for both ids. | Mirrors `package-access`'s message/message-id scheme; the suppression machinery is rule-name-generic already, so scoped directives work with zero new mechanism. |
| D-F9 | **Off/empty is free**: rule absent, `severity: "off"`, or an empty `files` list → the check phase does nothing for this rule. The D-F6 fallback resolution still runs (it is independently useful and costs one extra resolver call only for specifiers that failed the primary resolver). | Zero-cost-when-unused, like every other opt-in. |
| D-F10 | **Non-goals (v1)**: no access annotations *inside* non-TS files (no `/* @public */` scraping from CSS — many restricted formats are binary); no restriction of `node_modules`/external targets; no dynamic `import()` coverage (not extracted today, matching the reference plugin); no per-entry severity. | Keep v1 config-driven and file-level. Each of these is listed in §6 with what it would take, so scope grows deliberately. |

## 3. Configuration format

```jsonc
// .importlintrc.jsonc
{
  "rules": {
    "package-access": {
      "defaultImportability": "package"
      // packageDirectory / indexLoophole / filenameLoophole configured here
      // also define the package geometry file-access checks against (D-F5).
    },

    "file-access": {
      // "error" | "warn" | "off" — same as package-access.
      "severity": "error",

      // Ordered list; each entry maps glob pattern(s) to an importability.
      // Patterns match the *resolved target file's* project-relative path.
      // When several entries match one file, the LAST match wins.
      "files": [
        // A CSS module is only importable from inside its own package —
        // with default geometry: its own directory and below.
        { "pattern": "**/*.module.css", "importability": "package" },

        // Raw SQL is never importable; it's read by the migration runner only.
        { "pattern": "db/migrations/**/*.sql", "importability": "private" },

        // Override arm: the shared theme file is importable from anywhere
        // even though a broader *.css entry might say otherwise.
        { "pattern": "src/styles/global.css", "importability": "public" }
      ]
    }
  }
}
```

Shape notes:

- `pattern` accepts a single glob or an array of globs (`{"pattern":
  ["**/*.png", "**/*.svg"], "importability": "package"}`).
- Unknown keys anywhere in the `file-access` block are a hard load error
  (exit `2`), like the rest of the config (`deny_unknown_fields`).
- Files matched by **no** entry are unrestricted — exactly today's behavior,
  so an empty or absent `file-access` block changes nothing.

### Worked example

```
src/
├── components/
│   └── Button/
│       ├── Button.tsx           ── import styles from "./Button.module.css"   ✓
│       └── Button.module.css
└── pages/
    └── home.tsx                 ── import s from "../components/Button/Button.module.css"  ✗
```

With `{ "pattern": "**/*.module.css", "importability": "package" }` and no
`packageDirectory`, `Button.module.css`'s package is `src/components/Button/`:
the colocated component may import it; `home.tsx` gets

```
src/pages/home.tsx
  1:15  error  Cannot import a package-private file '../components/Button/Button.module.css'  file-access
```

With `packageDirectory: ["**/*.package"]` configured on `package-access`, the
same entry instead scopes each asset to its enclosing `*.package` boundary —
the geometry follows the project's, automatically (D-F5).

## 4. Semantics (precise)

For every module-referencing statement `S` (specifier `spec`) in a lint
target `importer`:

1. Resolve `spec` as today; if the primary resolver fails, run the D-F6
   fallback. Skip unless the result is `Asset(target)` or `Internal(target)`.
2. Compute `rel` = project-relative path of `target` (Node `path.relative`
   semantics, `/`-separated — same as `excludeSourcePatterns` matching).
3. Scan the `files` array in order; remember the **last** entry whose
   pattern(s) match `rel`. No match → allowed.
4. Apply that entry's `importability`:
   - `public` → allowed.
   - `package` → allowed iff `is_in_package(importer, target)` under the
     shared compiled geometry (D-F5); otherwise report `package-file` at
     `S`'s source-string span.
   - `private` → report `private-file` at `S`'s source-string span.
5. Suppression directives on `S`'s line (bare, or naming `file-access`)
   drop the diagnostic, as for any rule.

One diagnostic per statement, not per imported name: `import a, { b, c }
from "./x.css"` is one violation.

## 5. Implementation sketch

**core** (`crates/core`):

- `extract/`: record `(specifier, span)` for every module-referencing
  statement — a new `Vec<SpecifierRef>` on `FileModuleInfo` (D-F2). Covers
  side-effect imports, namespace imports, and `export * from`, which have no
  checked entry today.
- `resolve/`: add the fallback resolver (second `Resolver` instance, same
  tsconfig options, default conditions, no `extension_alias`) behind
  `ProjectResolver::resolve`; strip `?`/`#` suffixes before the fallback;
  add `Provenance::Asset(PathBuf)`. Fixpoint loop and `package-access`
  lookup ignore `Asset` by construction.
- `rule/`: new `file_access.rs` — compile the `files` entries once per
  check pass (invalid glob → stderr warning + never-matches, like
  `packageDirectory`), then the §4 loop; reuses `CompiledPackageOptions`
  built from `package-access`'s options.
- `config.rs`: extend the hand-written `Rules` deserializer with the
  `file-access` key (it currently hard-errors on any key but
  `package-access`); new `FileAccessRuleConfig { severity, files }`,
  `FileEntry { pattern: OneOrMany<String>, importability }`.

**cli** (`crates/cli`):

- `report.rs`: run the new check when severity ≠ off, mapping to rule id
  `file-access` (the existing severity/quiet/exit-code plumbing is
  rule-generic). `--report-unresolved` needs no change — resolvable assets
  simply stop being `Unresolved` (D-F6).
- `docs.rs`: `explain package-file` / `explain private-file`; extend the
  `config` topic.
- `init.rs`: add a commented-out `file-access` block with the
  `**/*.module.css` example to the scaffold template (the round-trip test
  keeps it honest).
- Watch/LSP: no structural changes (§1); diagnostics flow through the
  shared report path.

**docs**: README config reference + a short "Restricting non-TS imports"
section; concepts guide note; migration section gains a "beyond the ESLint
plugin" line (the reference plugin has no equivalent — worth calling out as
a reason to switch).

**tests**: config round-trip (including unknown-key rejection and
last-match-wins); resolver fallback (css/json/svg, tsconfig `paths` alias,
`?url` suffix, node_modules css → external, missing file → unresolved);
rule fixtures for each importability × statement form (default, named,
namespace, side-effect, `export * from`); loophole interplay
(`filenameLoophole` + `Button/styles.module.css`); suppression; CLI
end-to-end with `--format json`.

## 6. Future extensions (deliberately deferred)

- **`allowFrom` globs per entry** — arbitrary importer allowlists beyond
  package geometry (e.g. "only `*.stories.tsx` may import fixtures").
- **Per-rule `packageDirectory` override** on `file-access`, defaulting to
  the shared geometry, if a real project needs the two rules to disagree.
- **Dynamic `import()` extraction** — would benefit `package-access` equally;
  a separate decision.
- **Sidecar annotations** (e.g. `Button.module.css` + a marker in a
  neighboring file) as a config-free opt-out, if config-only proves too
  centralized in practice.
