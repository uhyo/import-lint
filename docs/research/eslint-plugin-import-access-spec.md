# Behavioral Specification: `eslint-plugin-import-access` v3.1.0

> Research appendix for [PLAN-v1.md](../PLAN-v1.md). Produced by analyzing the implementation and
> test suite at `~/repos/eslint-plugin-import-access` (not just the docs). This is the
> reference behavior that ImportLint must replicate.

## 1. Rules provided

The package exports exactly **one** ESLint rule, plus a TypeScript Language Service plugin (not an ESLint rule) that reuses the same core logic.

- **`import-access/jsdoc`** (`src/rules/jsdoc.ts`) — the only rule. `meta.type: "problem"`. Requires typed linting (`parserServices.program` from `@typescript-eslint/parser`); if type information isn't available it reports `no-program` and does nothing else.
- A bundled config `configs.all` sets `"import-access/jsdoc": "error"` with no options.
- Package also ships a **TS Language Service plugin** (`src/ts-server-plugin/index.ts`) that wraps `getCompletionsAtPosition` and `getCodeFixesAtPosition` to filter out auto-import suggestions that would violate the same importability rule. It calls the exact same `checkSymbolImportability` core function. Irrelevant to a CLI reimplementation except as evidence the core check is meant to be host-agnostic.

There is no separate rule for anything else — everything is this one rule.

## 2. Full configuration surface

Options object (2nd array element of the ESLint rule config), all optional, `additionalProperties: false`:

```ts
type JSDocRuleOptions = {
  indexLoophole: boolean;               // default: true
  filenameLoophole: boolean;            // default: false
  defaultImportability: "public" | "package" | "private"; // default: "public"
  treatSelfReferenceAs: "internal" | "external";           // default: "external"
  excludeSourcePatterns?: string[];     // default: []
  packageDirectory?: string[];          // default: undefined (== ["**"], every directory is a package)
};
```

- **`indexLoophole`** — when true, an exporter file matching `/\/index\.[cm]?[jt]sx?$/` (i.e. `index.ts`, `index.tsx`, `index.js`, `index.cjs`, `index.mjs`, `index.jsx`) has that suffix stripped before computing its "package directory," effectively promoting `sub/index.ts`'s exports to live in `sub`'s *parent* directory (same level as a hypothetical `sub.ts`). **Only one level** — it does not recurse (an `index.ts` re-exporting another `index.ts` does not compound).
- **`filenameLoophole`** — when true, a file `sub.ts` may import package-private exports from any file directly inside directory `sub/` (siblings of `sub.ts`, not deeper). Directory name must equal the importer's basename without extension.
- **`defaultImportability`** — governs the effective access level for exports with **no** matching JSDoc tag at all (including exports that have a JSDoc comment but none of the recognized tags).
- **`treatSelfReferenceAs`** — controls whether Node.js "self-reference" imports (importing your own package by its `package.json` `"name"`) bypass the check entirely (`"external"`, default) or are checked as if they were relative/internal imports (`"internal"`).
- **`excludeSourcePatterns`** — array of minimatch glob patterns matched against the **exporter**'s file path, relative to `program.getCurrentDirectory()` (the project root), using `{ dot: true }`. First matching pattern short-circuits the whole check (import is always allowed), checked *before* the external-library check and before self-reference/JSDoc logic.
- **`packageDirectory`** — array of minimatch glob patterns (matched against **both** the bare directory basename and the path relative to the project directory) that define which directories are "package boundary" directories. Negation patterns (`!pattern`) exclude a directory from being a boundary even if a positive pattern also matches it (if ANY negation pattern matches → not a package dir, short-circuit; otherwise a package dir if ANY positive pattern matched). If unset/empty, every file's own parent directory is its package directory. When set, `findPackageDirectory()` walks upward from the file's directory until it finds an ancestor directory satisfying `isPackageDirectory()`; if none found before the filesystem root, it falls back to `path.dirname(filePath)`.

There is no `access` shorthand config, no per-file overrides, and no way to vary options by glob (only multiple ESLint config blocks).

## 3. Core semantics of `@package` / `@private` / `@public`

### 3.1 Where the JSDoc annotation is looked for

`findExportedDeclaration(rawDecl)` (`src/utils/findExportableDeclaration.ts`) walks **up** the TS AST from the raw declaration node of the resolved symbol until it hits the source file, looking for the nearest ancestor that is one of:

- `FunctionDeclaration`, `ClassDeclaration`, `VariableStatement`, `TypeAliasDeclaration`, `InterfaceDeclaration` — **must** carry an `ExportKeyword` modifier, else `undefined` (not a qualifying export) is returned and the whole check is skipped (no error, silently ignored).
- `ExportDeclaration` (`export { x }`, `export { x } from "./y"`) or `ExportAssignment` (`export default <expr>;`, `export = x;`) — returned unconditionally.

JSDoc tags are gathered from **two sources and concatenated**:
1. `exportedSymbol.getJsDocTags(checker)` — TS's own Symbol API.
2. `getJSDocTags(decl)` (via `tsutils.getJsDoc`) — tags found by walking the JSDoc comment blocks directly attached to the `decl` node found above.

`getAccessOfJsDocs` scans this combined list **in order** and returns on the **first** matching tag:
- `@package` → `"package"`
- `@private` → `"private"`
- `@public` → `"public"`
- `@access package` / `@access private` / `@access public` (tag name `access`, comment text exactly one of these three strings) → respective value
- No recognized tag found anywhere → falls through to `defaultImportability`.

If there is **no JSDoc at all** on the declaration, a separate branch switches directly on `defaultImportability` (functionally equivalent).

**Only the JSDoc found at the single alias hop being checked is considered — chains are not transitively resolved to the ultimate original declaration.** This is the most important non-obvious behavior (see §4/§6).

### 3.2 Export forms supported

The rule's visitor only listens on three node types: `ImportSpecifier`, `ImportDefaultSpecifier`, `ExportSpecifier` (with a module specifier — i.e. `export { x } from "y"`, not bare `export { x }`).

| Form | Checked? | Notes |
|---|---|---|
| `import { x } from "./m"` | Yes | `ImportSpecifier` |
| `import { x as y } from "./m"` | Yes | reported name uses the **original exported name**, not local alias |
| `import x from "./m"` (default) | Yes | `ImportDefaultSpecifier`; reported identifier name is `"default"` |
| `import { default as x } from "./m"` | Yes | treated as ImportSpecifier named `default` |
| `export { x } from "./m"` | Yes | `ExportSpecifier` with moduleSpecifier; `reexport=true` → uses `...:reexport` messageIds |
| `export { x as y } from "./m"` | Yes | reported name is original export name |
| Local `export { x };` (no `from`) | Not directly | skipped by the `ExportSpecifier` handler; but if `x` was itself imported, that import's own specifier is separately checked. A local export whose binding was declared in the *same file* is never checked — intentional. |
| `export * from "./m"` | NOT checked | No `ExportAllDeclaration` visitor. Barrel re-exports via `export *` bypass the rule at the re-export statement itself. |
| `import * as ns from "./m"` | NOT checked | No `ImportNamespaceSpecifier` visitor. `ns.privateExport` accesses are never flagged. |
| Dynamic `import("./m")` | NOT checked | |
| `import x = require("./m")` (TS `import=`) | NOT checked | |
| CommonJS `require()` | NOT checked | |
| `export default class {}` / `export default function foo(){}` | Yes | reported name `"default"` |
| `export default <expression>;` | Yes | `ExportAssignment` node |
| Declaration merging (e.g. `interface X {}` twice) | Partial | Only `exportedSymbol.declarations?.[0]` (the **first** declaration) is consulted for the AST walk-up, though `symbol.getJsDocTags()` aggregates across all declarations. |
| `import type { X } from "./m"` / `export type { X }` | Yes (same code path) | No special-casing of `importKind`. Not covered by any test in the suite though. |

### 3.3 "Same package" directory algorithm (`isInPackage`)

Given `importer` (linted file's absolute path) and `exporter` (absolute path of the file containing the qualifying declaration found in §3.1):

1. If `indexLoophole`: if `exporter` matches `/\/index\.[cm]?[jt]sx?$/`, strip that suffix (e.g. `.../sub/index.ts` → `.../sub`).
2. Compute `importerPackageDir` and `exporterPackageDir`:
   - If `packageDirectory` option is unset/empty: `path.dirname(file)` (after the loophole substitution above for the exporter).
   - If set: walk up from `path.dirname(file)` through ancestors, testing each directory's **basename** and its **path relative to the project directory** against each pattern (any `!pattern` negation matching disqualifies that directory) until one qualifies, or fall back to `path.dirname(file)`.
3. **Same package**: `importerPackageDir === exporterPackageDir` (string equality) → allowed.
4. **Filename loophole** (only if option true, checked with the *original* file paths, not the package-dir-adjusted ones): `path.relative(dirname(importer), dirname(exporter)) === basename(importer, ext(importer))`. One level deep only — `sub/deep/x.ts` does **not** satisfy this.
5. **Ancestor/descendant check**: `rel = path.relative(exporterPackageDir, importerPackageDir)`; allowed if `rel !== "" && !rel.startsWith("..") && !path.isAbsolute(rel)` — importer's package directory is a **descendant** of the exporter's package directory. A child package may import package-private exports of any of its ancestor packages, but not the reverse, and not siblings/cousins.

### 3.4 `@private` is absolute

Regardless of directory relationship, `access === "private"` always yields the `"private"`/`"private:reexport"` error — directory checks are skipped entirely. Only `"package"` goes through the directory algorithm. `"public"` always allows.

## 4. TypeScript type-checker usage — what must be reimplemented

Exact API calls in `src/rules/jsdoc.ts` and `src/core/checkSymbolmportability.ts`:

1. **`parserServices.esTreeNodeToTSNodeMap.get(node)`** — maps the ESTree node to the TS AST node.
2. **`checker.getSymbolAtLocation(tsNode.name)`** — resolves the identifier at the specifier to the **alias symbol** created by the import binding.
3. **`checker.getImmediateAliasedSymbol(symbol)`** — critically, this is the **single-hop** alias resolution, *not* `getAliasedSymbol` (which fully dereferences to the ultimate original declaration). Returns `undefined` if the symbol has no alias target (early-return guard).
   - **This is the single most consequential fact for a reimplementation.** The tool checks JSDoc/importability at the position **one hop away** from the import/export site being linted, not at the terminal original declaration. Concretely:
     - `import { x } from "./mod"` — checks `mod.ts`'s own export of `x`. If that is itself a re-export (`export { x } from "./inner"`), the declaration found is the `ExportDeclaration` node **in `mod.ts`**, and its JSDoc — if none — falls back to `defaultImportability` **even if `inner.ts`'s original declaration of `x` has `@package`**. Verified by test: `fixtures/project/src/reexport/sub2/index.ts` re-exports `subBar` from `./bar` (no JSDoc on the re-export line) even though `bar.ts`'s `subBar` has `@package`; importing it from `reexport/useBar.ts` produces **no error**.
     - Conversely, if a re-export statement **adds its own JSDoc tag**, that tag governs, and the file used for the directory check is the *re-exporting* file. Verified: `reexport/sub3/index.ts` (`/** @private */ export { subBaz } from "./bar";`) causes a **private** error even though `bar.ts` only has `@package`.
     - A **local** re-export of an imported binding (`import { subFoo } from "./foo"; /** @access package */ export { subFoo };`) — the alias's declaration is the local `ExportSpecifier`; the walk-up finds that local `ExportDeclaration`, whose own JSDoc is checked, with the **exporting file being where the `export { subFoo };` line lives**, not `foo.ts`.
   - Implication: build (per module) a **local export table**: for each exported name, record (a) the export statement/declaration node introducing it in *this* file, and (b) its module specifier if it's a re-export. "One hop" = look at *that* node's JSDoc; the "exporter file" for the directory check is **always the file containing the statement identified by the walk-up** — never the transitively-original file, and never more than one hop even if that hop is itself a passthrough re-export with no own JSDoc (the check terminates at the first hop regardless).
4. **`symbol.getJsDocTags(checker)`** — unioned with a from-scratch AST walk. A reimplementation needs **one** correct JSDoc-comment-to-declaration association (comment block immediately preceding the export statement/declaration node); the dual-source concat is redundancy, not semantics.
5. **`program.isSourceFileFromExternalLibrary(sourceFile)`** — true if TS considers the file part of an external dependency, **based on resolution provenance, not path pattern-matching for `/node_modules/`.** This matters for **workspace symlinks**: the `packages/workspaces/*` fixtures are npm-workspaces packages symlinked into `node_modules/@fixture-package-workspace/*`; TypeScript resolves symlinks to their real path (so the filename does **not** contain `/node_modules/`), yet the file is still classified as "external" and exempt (see `src/__tests__/library.ts`, "Workspace modules (symlink)" — all pass with **no errors** even with `defaultImportability: "package"`). A naive `path.contains("node_modules")` check **will misclassify workspace-symlinked packages as internal source**. Track *how* a specifier was resolved (bare-specifier lookup through a `node_modules` directory) rather than pattern-matching the final path.

**What is genuinely type-system-dependent: nothing.** The tool never inspects *types* (no narrowing, no generics, no inference). It only uses the checker for (a) module/import binding resolution and (b) JSDoc tag retrieval — both replicable via pure syntactic module-graph analysis. Handle carefully without tsc:
- External-library determination via resolution provenance.
- The module resolution algorithm itself (must approximate the project's `moduleResolution` mode; `paths`/`baseUrl`, `exports`/`main`/`types`/`typesVersions`).
- Ambient module declarations (`declare module "x"` in a project `.d.ts`) resolve to project-local files even though the specifier looks like a bare/library import (see `fixtures/project/src/exclude-patterns/types/types.d.ts` — ambient `declare module "generated-package"` resolves to an in-project `.d.ts`, correctly treated as **internal**, requiring `excludeSourcePatterns` to opt out).

## 5. Path/module resolution behavior

- Resolution is fully delegated to the TypeScript `Program` configured by the linted project's real `tsconfig.json`. The test fixture project uses `"module": "node16", "moduleResolution": "node16"`. There is no bespoke resolution logic in the plugin.
- `tsconfig.json` `paths`/`baseUrl` are not exercised in tests, but transparently work via the Program; a reimplementation needs a `paths`-aware resolver (plus `baseUrl`, `extends` chains, project references) to match.
- **Third-party packages** are exempted regardless of entry-point shape (`main`+`types`, `types`-only, `exports`-field-only), for both true `node_modules`-installed and npm-workspaces-symlinked packages, including subpath imports (`pkg/sub`). Tested exhaustively in `src/__tests__/library.ts`.
- **Node builtin modules** (`"path"`, `"node:path"`) are exempt (resolve to `@types/node`, which counts as external).
- **Declaration files (`.d.ts`)** that are part of *your own project source* are ordinary internal exporters subject to the full check — a common real-world case being generated `.d.ts` files that the docs recommend excluding via `excludeSourcePatterns`.
- `context.filename` is always used as an **absolute path**; relative-path comparisons throughout assume absolute inputs.

## 6. Notable/tricky test-suite edge cases

From `src/__tests__/*.ts` (fixture paths relative to `src/__tests__/fixtures/project/`):

- **Basic declaration kinds** all behave identically — classes, functions, `const`/`let`/`var` (including destructuring patterns, e.g. `export var { fooDestructed = 0 } = {};`), type aliases, interfaces. Same-directory imports always pass; sub-directory imports of package-private members fail with one diagnostic per specifier.
- **Default exports**: `export default "I am bar";` with `@package` vs. no JSDoc — the latter behaves per `defaultImportability`. Both `import bar1 from "./sub/bar"` and `import { default as bar2 } from "./sub/bar"` are checked and independently reported.
- **Re-export chain "reset" semantics** (the crux, see §4):
  - Local re-export with distinct JSDoc per statement: `export { subFoo }; /** @private */ export { subFoo as subFooPrivate };` — two access levels for the same value under two names.
  - `export { subBar } from "./bar";` with **no** JSDoc "erases" the `@package` on the original declaration.
  - `export { subBaz } from "./bar";` with own `/** @private */` — private wins, checked at the re-export site's directory.
  - Re-exporting a package-private export cross-directory (`reexportFromSubFoo.ts` → `./sub/foo`) fails with `package:reexport`.
  - Re-exporting **through** an `index.ts` (`reexportFromSubIndex.ts` → `./sub/index`, where `sub/index.ts` has `/** @package */ export { subFoo } from "./foo";`) **succeeds** because `indexLoophole` makes `sub/index.ts`'s effective directory its parent. `indexLoophole: false` flips this to an error.
  - `filenameLoophole` with re-exports: `reexport4/filenameLoophole/sub.ts` (`export { subFoo } from "./sub/foo";`) succeeds because basename `sub` matches directory `sub/`; identical content in `sub2.ts` fails. The loophole is purely based on the *importing file's own name*.
  - `defaultImportability: "package"` + `@public` target still allows cross-directory import; an unannotated sibling fails. External imports never produce diagnostics even mixed into the same file.
- **`directory-structure.ts`**: sibling/cousin sub-packages cannot import each other's package-private exports; descendants **can** import ancestors' (`sub/sub2/parentUser.ts` importing from `sub/pkg.ts` passes).
- **`package-directory.ts`**: without the option, `_internal` dirs are normal boundaries. With `["**", "!**/_internal"]`, `_internal` merges into its parent's package (bidirectionally), but cross-package reach-ins still fail. With `["src/package-directory/packages/*"]`, deep subdirectories within a matched package are freely importable inside it, but other packages cannot reach in. `filenameLoophole` composes with `packageDirectory` (checked against raw paths, independent of package-dir resolution).
- **`self-reference.ts`**: importing via the package's own `"name"` + `"exports"` subpath (`@uhyo/project/self-reference`) succeeds under `treatSelfReferenceAs: "external"`, fails under `"internal"` (with `defaultImportability: "private"`). `lookupPackageJson` walks up from the **importer**, finds the nearest `package.json`, and compares `specifier === name || specifier.startsWith(name + "/")` — purely string-based.
- **`exclude-patterns.ts`**: ambient-module `.d.ts` inside the project is checked like normal internal source by default; `excludeSourcePatterns` (matched against the exporter's project-relative path) bypasses.
- **`library.ts`**: exhaustive external matrix — builtins, real node_modules package, three entry-point shapes × {installed, workspace-symlinked}, subpath variants. All zero diagnostics even under `defaultImportability: "package"`.

## 7. Diagnostics — exact messages, messageIds, reported node

```ts
type MessageId = "no-program" | "package" | "package:reexport" | "private" | "private:reexport";
```

| messageId | Message template | Reported node |
|---|---|---|
| `no-program` | `Type information is not available for this file. See https://typescript-eslint.io/getting-started/typed-linting/ for how to set this up.` | the specifier node |
| `package` | `Cannot import a package-private export '{{ identifier }}'` | the import specifier node |
| `package:reexport` | `Cannot re-export a package-private export '{{ identifier }}'` | the `ExportSpecifier` node |
| `private` | `Cannot import a private export '{{ identifier }}'` | the import specifier node |
| `private:reexport` | `Cannot re-export a private export '{{ identifier }}'` | the `ExportSpecifier` node |

- `{{ identifier }}` is the **name of the target export at the immediate alias hop** (e.g. `"default"` for default exports), **not** the local alias (`import { barPackage as renamed }` reports `'barPackage'`).
- Exactly one diagnostic per specifier node; `import { a, b, c } from "./x"` can produce 0–3 diagnostics, each spanning just that specifier (including the `as alias` suffix).
- Forms outside the three visited node types produce nothing, silently.
- When the alias hop or the walk-up fails (no alias, no declarations, no qualifying export statement), the check silently passes.

## 8. Explicit flags for the Rust port

1. **One-hop alias resolution** (§4.3) is the largest correctness risk; getting "one hop, not full transitive resolution" wrong silently diverges on any re-export chain.
2. **Module resolution** must match the target project's `moduleResolution` mode closely enough (`paths`/`baseUrl`, `main`/`types`/`exports`/`typesVersions`).
3. **External-vs-internal classification** must be resolution-provenance-based, not path-substring-based (workspace symlinks).
4. **Ambient module declarations** must resolve to the declaring project file (internal).
5. **JSDoc-to-declaration association** is purely syntactic: the JSDoc block immediately preceding the statement identified by the walk-up.
6. **No type inference needed anywhere.**

Primary reference files: `src/core/checkSymbolmportability.ts`, `src/utils/isInPackage.ts`, `src/utils/findExportableDeclaration.ts`, `src/utils/getAccessOfJsDocs.ts`, `src/utils/getJSDocTags.ts`, `src/core/lookupPackageJson.ts`, `src/rules/jsdoc.ts`.
