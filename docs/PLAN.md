# ImportLint — Rule Rename Plan (M11): `jsdoc` → `package-access`

Prior plans: [`PLAN-v1.md`](./PLAN-v1.md) (M0–M7), [`PLAN-npm.md`](./PLAN-npm.md)
(N1–N3), [`PLAN-lsp.md`](./PLAN-lsp.md) (M8), [`PLAN-init.md`](./PLAN-init.md)
(M9), [`PLAN-docs.md`](./PLAN-docs.md) (M10).

**Problem:** the only rule is named `jsdoc` — inherited from the ESLint plugin's
`import-access/jsdoc`. That names the *mechanism* (JSDoc tags) rather than what
the rule enforces, and clashes with the newcomer framing M10 established
(packages, boundaries, importability). A newcomer reading
`"rules": { "jsdoc": ... }` learns nothing about what the rule does.

**Goal:** the rule is named **`package-access`** in the config file, in every
diagnostic, and in all docs; a config still using `jsdoc` fails fast with a
message that says exactly what to change. Ships in the next release (v0.1.3),
pre-1.0, as a deliberate breaking change.

## 1. Decisions (locked; naming and back-compat chosen by the project owner)

| # | Decision | Rationale |
|---|---|---|
| D-R1 | Config key `rules.jsdoc` → **`rules.package-access`**. Internal Rust names follow (`JsdocRuleConfig` → `PackageAccessRuleConfig`, `JsdocRuleOptions` → `PackageAccessRuleOptions`, etc.), **except** `extract/jsdoc.rs` and other mechanism-level code that genuinely parses JSDoc — the tag syntax is still JSDoc; only the *rule identity* is renamed. | Pre-1.0, `import-lint-core` has no API stability guarantee (RELEASING.md); leaving internal names stale buys nothing. The extract module is correctly named for what it does. |
| D-R2 | Displayed rule ID becomes **`package-access`** (bare, matching the config key) in all four surfaces: `pretty` output, `--format json` `ruleId`, `--format github` annotations, and the LSP diagnostic code. The `import-access/` prefix was ESLint-plugin namespacing and carries no meaning here. | One name everywhere: what you write in the config is what you see in the diagnostic. |
| D-R3 | A config containing `rules.jsdoc` is a **hard load error** (exit `2`) with a dedicated hint — `the rule "jsdoc" was renamed to "package-access"; update your config` — not the generic unknown-field error. No alias. | Owner's call: clean break while pre-1.0. The dedicated message makes the break one obvious edit. **Gotcha:** serde `flatten`+`deny_unknown_fields` interplay (serde#1600) is why the rule config already has a hand-written two-pass `Deserialize` — the `jsdoc` special-case must actually fire, with a test proving the exact message, not rely on default unknown-field handling. |
| D-R4 | Docs sweep under M10's D-D3 rule: every pasted diagnostic containing `import-access/jsdoc` (README example, all three guides, output-formats section) is re-generated from a real run of the renamed binary — not string-substituted. The Migration section gains two bullets: config key mapping (`import-access/jsdoc` ESLint rule → `"package-access"` here) and the `ruleId` change for anyone's CI filters. init.rs templates rename the key; their "identical options to eslint-plugin-import-access's `import-access/jsdoc` rule" comments stay (still true, and now they carry the lineage the rule name no longer does). | The M10 docs are the product surface this rename exists to serve; letting them drift on day one would be absurd. |
| D-R5 | Conformance snapshots under `tests/conformance/` are the **oracle** (real ESLint-plugin output) and are never regenerated. If the harness compares rule IDs, it gains a normalization step; expected-output fixtures that are *ours* (not oracle) are updated. | The oracle's value is that it wasn't produced by us. |

## 2. Blast radius (from the pre-plan grep)

- `crates/core`: `config.rs` (key + hand-written Deserialize + D-R3 error),
  `lib.rs` re-exports, `rule/options.rs`, `rule/mod.rs` naming.
- `crates/cli`: `report.rs:151`, `output/pretty.rs:117`, `output/eslint_json.rs:126`,
  `output/github.rs:55` (hardcoded rule ID ×4 + their tests), `setup.rs`,
  `init.rs` templates ×3, LSP diagnostic code (check `lsp/convert.rs`).
- Tests: `cli.rs`, `lsp.rs`, `watch.rs`, `conformance.rs` (D-R5),
  `init.rs` round-trip assertions, `core/tests/extract_forms.rs`.
- Docs: `README.md`, `docs/guides/{concepts,tutorial,adoption}.md`
  (config blocks, prose "the `jsdoc` rule", and every pasted diagnostic),
  `docs/RELEASING.md` prose-sync item mentions. `npm`/vscode READMEs don't
  name the rule — verify, don't assume.

## 3. Milestone

**R1 (single):** everything in §1–§2, plus: a config-load test asserting the
D-R3 message verbatim; `cargo fmt`/`clippy -D warnings`/`cargo test --workspace`
green; `node --test npm/import-lint/test/*.test.js` green; docs diagnostics
re-verified per D-R4. Exit: `import-lint init --preset standard && import-lint`
green in a temp dir with the new key; a config with `rules.jsdoc` exits `2`
printing the hint; `grep -r "import-access/jsdoc"` in the repo hits only the
lineage comments/migration prose that intentionally reference the ESLint rule,
and `"jsdoc"` as a config key appears nowhere outside archived plans.

## 4. Out of scope

- Any alias/deprecation window for `jsdoc` (owner chose hard break).
- Renaming JSDoc-mechanism internals (`extract/jsdoc.rs`) or the accepted tag
  spellings (`@package`, `/** @access package */` are syntax, not rule name).
- Renaming diagnostics' message texts (still exact ESLint-plugin parity).
- A config auto-migrator (`import-lint migrate-config`) — the fix is one line.
