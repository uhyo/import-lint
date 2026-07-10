# Spike S4 — `export =` (TSExportAssignment) on the exporter side

Method: same harness/process as [spike-s1-star-exports.md](spike-s1-star-exports.md)
— temporary fixtures under `src/__tests__/fixtures/project/src/__spike__/s4_*` in the
reference repo, temporary `src/__tests__/__spike__.ts` driver, run via
`npx vitest run src/__tests__/__spike__.ts`. All temp files deleted afterward;
`git status --porcelain` confirmed clean.

Reference-repo internals consulted: `src/rules/jsdoc.ts` (`checkSymbol`, which reads
`exsy.name` for the reported identifier) and
`src/utils/findExportableDeclaration.ts` (`findExportedDeclaration` returns
immediately when the walked-up node satisfies `isExportAssignment(node)`, without
walking further to any declaration the assigned expression might reference).

## Case 1 — JSDoc directly on the `export =` statement

```ts
// mod/mod.ts
const someValue = { foo: 1 };
/**
 * @package
 */
export = someValue;

// user/importer.ts   (sibling directory of mod/)
import mod from "../mod/mod";
console.log(mod);
```

Observed:
```json
[{"message":"Cannot import a package-private export 'export='","messageId":"package", ...}]
```

## Case 2 — JSDoc on the declaration, not on `export =`

```ts
// mod/mod.ts
/**
 * @package
 */
const someValue = { foo: 1 };
export = someValue;

// user/importer.ts   (sibling directory of mod/)
import mod from "../mod/mod";
console.log(mod);
```

Observed: `[]` (no error).

## Case 3 — same-directory control (JSDoc on `export =`, `@package`)

```ts
// mod.ts
const someValue = { foo: 1 };
/**
 * @package
 */
export = someValue;

// importer.ts   (same directory as mod.ts)
import mod from "./mod";
console.log(mod);
```

Observed: `[]` (no error) — confirms the directory check behaves normally for
`export =` exports (this isn't unconditionally erroring; case 1's error is
specifically due to the sibling-directory relationship).

## Case 4 — `@private` on `export =`, different directories

```ts
// mod/mod.ts
const someValue = { foo: 1 };
/**
 * @private
 */
export = someValue;

// user/importer.ts   (sibling directory of mod/)
import mod from "../mod/mod";
console.log(mod);
```

Observed:
```json
[{"message":"Cannot import a private export 'export='","messageId":"private", ...}]
```

## Answer

- `import mod from "./mod"` against a module using `export = someValue` (under
  `esModuleInterop`) **is checked**: `checker.getImmediateAliasedSymbol` resolves
  the default-import binding to TypeScript's synthetic symbol for the export
  assignment, whose `.name` is the literal string **`"export="`** (TypeScript's
  `InternalSymbolName.ExportEquals`) — **not** `"default"`. This is the identifier
  reported in the diagnostic message (`Cannot import a {private,package-private}
  export 'export='`), and it's the name a Rust port's `export_table` should use as
  the key for an `export =` entry.
- Only JSDoc placed **directly on the `export = expr;` statement** governs. JSDoc on
  the declaration the expression happens to reference (e.g. a `const` a few lines
  above) is **not** picked up — `findExportedDeclaration`'s AST walk-up starts from
  the export-assignment symbol's own declaration node (the `ExportAssignment` node
  itself, i.e. `export = someValue;`) and returns as soon as it recognizes that node
  kind; it never inspects `someValue`'s originating declaration. This mirrors the
  ordinary "one-hop, from the exporter statement's own JSDoc only" rule elsewhere in
  the plugin — there is nothing `export =`-specific here except that the hop lands on
  the `ExportAssignment` node rather than a `VariableStatement`/`FunctionDeclaration`/etc.
- The in-package directory check behaves normally: `@package` passes when importer
  and the file containing `export =` share a directory (case 3) and fails across
  sibling directories (case 1); `@private` is absolute regardless of directory
  (case 4), consistent with §3.4 of the spec.

**For the Rust port:** treat `export = <expr>;` as an ordinary export-table entry
under the name `"export="`, whose JSDoc source is the `export =` statement's own
leading comment (never the RHS declaration's). `import x from "./mod"` (default
import syntax) against such a module should look up `"export="` in the target's
export table — i.e. the port's module-resolution/binding layer needs to know that a
default import against an `export =` module binds to the `"export="` key, distinct
from an ordinary `export default` (which — per the existing spec §3.2 — uses the key
`"default"`).

## Not determined / not applicable

- `import { something } from "./mod"` against an `export =` module was not tested.
  Structurally, `export =` replaces the module's entire export surface with a
  single value; TypeScript does not synthesize named bindings for properties of
  that value for `import { x } from "./mod"` syntax (that would require
  `import mod = require("./mod")` plus a subsequent property access, which is a
  node type — `TSImportEqualsDeclaration` — the reference plugin's visitor
  explicitly does not listen on per §3.2 of the spec ("`import x =
  require(...)` NOT checked")). This case doesn't arise for the Rust port's
  purposes and was not run.
