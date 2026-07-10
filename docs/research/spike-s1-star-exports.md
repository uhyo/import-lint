# Spike S1 — `export * from` (star-export) semantics

Method: temporary fixtures added under `src/__tests__/fixtures/project/src/__spike__/`
in the reference repo (`eslint-plugin-import-access`), driven by a temporary
`src/__tests__/__spike__.ts` that calls the existing `getESLintTester().lintFile()`
harness (same harness used by `src/__tests__/reexport.ts`) and dumps the raw
`LintMessage[]` for each fixture's `importer.ts`. Run with
`npx vitest run src/__tests__/__spike__.ts`. All temp files were deleted afterward;
`git status --porcelain` in the reference repo is clean (verified).

Reference-repo internals consulted to interpret results: `src/rules/jsdoc.ts`
(`checker.getImmediateAliasedSymbol(symbol)` — one hop from the import's alias
symbol), `src/core/checkSymbolmportability.ts`, and
`src/utils/findExportableDeclaration.ts` (`findExportedDeclaration`: walks up from
`exsy.declarations[0]` until it hits a `FunctionDeclaration` / `ClassDeclaration` /
`VariableStatement` / `ExportDeclaration` / `ExportAssignment` / `TypeAliasDeclaration`
/ `InterfaceDeclaration`).

## Q1 — Is `@package`/`@private` enforced through a star export at all?

Fixture (`s1_q1_private/`, all three files in the same directory):

```ts
// inner.ts
/**
 * @private
 */
export const x = 1;

// barrel.ts
export * from "./inner";

// importer.ts
import { x } from "./barrel";
console.log(x);
```

Observed: `[{"message":"Cannot import a private export 'x'","messageId":"private", ...}]`.

The parallel `@package` fixture (`s1_q1_package/`) produced `[]`, but that fixture is
directory-uninformative by construction (importer, barrel, and inner are all in the
same directory, so `isInPackage` would pass regardless of which file is "the
exporter"). The `@private` case is directory-independent (private is absolute, §3.4
of the spec) and is therefore the discriminating test.

**Answer:** Yes — the check *is* applied through `export * from`. TypeScript's
`getImmediateAliasedSymbol` resolves a name that only exists via a star re-export
directly to the original declaring symbol; there is no intermediate alias/declaration
node in `barrel.ts` for the checker to stop at (unlike a named re-export, which
creates an explicit `ExportSpecifier`/`ExportDeclaration` node in the re-exporting
file). Star exports are effectively "zero-hop" / fully transparent, in contrast to
named re-exports which are exactly one hop.

## Q2 — Which file/directory is "the exporter" for the in-package check?

Two fixtures, using sibling (non-ancestor/descendant) directories so the two
hypotheses ("exporter = inner's dir" vs "exporter = barrel's dir") give different
predictions:

**Q2a** — importer colocated with barrel, inner in a sibling dir:

```ts
// innerdir/inner.ts
/** @package */
export const x = 1;

// barreldir/barrel.ts
export * from "../innerdir/inner";

// barreldir/importer.ts  (same dir as barrel.ts)
import { x } from "./barrel";
console.log(x);
```

Observed: `[{"message":"Cannot import a package-private export 'x'","messageId":"package", ...}]`
(i.e. treated as **not** in-package).

**Q2b** — importer colocated with inner, barrel in a sibling dir:

```ts
// innerdir/inner.ts
/** @package */
export const x = 1;

// barreldir/barrel.ts
export * from "../innerdir/inner";

// innerdir/importer.ts  (same dir as inner.ts)
import { x } from "../barreldir/barrel";
console.log(x);
```

Observed: `[]` (no error, i.e. treated as in-package).

**Answer:** The exporter for the directory check is **`inner.ts`'s directory** — the
file containing the *original* declaration — not `barrel.ts`'s directory. Star
exports fully flatten to the terminal declaration for purposes of both the JSDoc
lookup (Q1) and the directory check (Q2); the re-exporting file's location is
irrelevant.

## Q3 — Chained star exports (`barrel → mid → inner`)

Enforcement (private, directory-uninformative — all three files share a directory):

```ts
// inner.ts
/** @private */
export const x = 1;
// mid.ts
export * from "./inner";
// barrel.ts
export * from "./mid";
// importer.ts
import { x } from "./barrel";
console.log(x);
```

Observed: `private` error reported — confirms the resolution isn't limited to a
single `export *` hop; it walks an arbitrary chain (as expected, since TS resolves
this at the "exports of module" level, not via an explicit alias-symbol chain).

**Directory triangulation** (`s1_q3_dircheck/`), three directories `dirA` (barrel),
`dirB` (mid), `dirC` (inner, `@package`), with importers colocated with `mid` and
with `inner` respectively:

```ts
// dirC/inner.ts
/** @package */
export const x = 1;
// dirB/mid.ts
export * from "../dirC/inner";
// dirA/barrel.ts
export * from "../dirB/mid";
// dirB/importerMid.ts (same dir as mid.ts)
import { x } from "../dirA/barrel";
// dirC/importerInner.ts (same dir as inner.ts)
import { x } from "../dirA/barrel";
```

Observed: `importerMid` → `package` error (not in-package); `importerInner` → `[]`
(in-package).

**Answer:** Even through a 2-hop `export *` chain, the exporter directory is
unambiguously `inner.ts`'s directory (the terminal original declaration), not
`mid.ts`'s or `barrel.ts`'s. This rules out "exporter = the immediate re-exporting
file nearest the import" as well as "exporter = the first file in the chain";
it is always the file with the actual declaration.

## Q4 — Precedence: star export vs. an explicit re-export of the same name

```ts
// inner.ts
/** @package */
export const x = 1;
// inner2.ts
/** @private */
export const x = 2;
// barrel.ts
export * from "./inner";
export { x } from "./inner2";       // no JSDoc on this line
// importer.ts
import { x } from "./barrel";
console.log(x);
```

Observed: `[]` (no error).

**Answer:** An explicit named re-export **wins over** a star export of the same
name (ordinary ES module shadowing semantics — TypeScript's checker resolves `x` to
the explicit `export { x } from "./inner2"` binding, not through the star export).
Crucially, once that explicit re-export is selected, ordinary **one-hop** semantics
apply from there: since the `export { x } from "./inner2"` line in `barrel.ts` has
no JSDoc of its own, the check falls back to `defaultImportability` (public) and
does **not** descend into `inner2.ts`'s `@private` declaration, even though `x`'s
star-exported sibling from `inner.ts` would have been enforced. This means: **a
name resolves via a star export only if no explicit local declaration or named
(re-)export of that name exists in the same file** — i.e. `lookup()` should check
the local export table (including named re-exports) before falling through to
`star_exports` descent, exactly as already specified in PLAN.md §2.3.

## Q5 — `export * as ns from "./inner"`

Plain case — no JSDoc anywhere on the `export * as ns` statement, importer accesses
`ns.x`:

```ts
// inner.ts
/** @package */
export const x = 1;
// barrel.ts
export * as ns from "./inner";
// importer.ts
import { ns } from "./barrel";
console.log(ns.x);
```

Observed: `[]` (no error).

JSDoc placed directly on the `export * as ns from` statement:

```ts
// barrel.ts
/**
 * @private
 */
export * as ns from "./inner";
```

Observed: `[{"message":"Cannot import a private export 'ns'","messageId":"private", ...}]`
(reported identifier is `'ns'`, not `'x'`).

**Answer:** `ns.x` property access is never checked (consistent with §3.2 of the
spec: `import * as ns from "./m"` / namespace member access is out of scope — the
rule only ever inspects the *binding* named at an `ImportSpecifier` /
`ImportDefaultSpecifier` / re-exporting `ExportSpecifier`, never a subsequent
property access). The `ns` binding created by `export * as ns from "./inner"` is
itself an ordinary checkable export: with no JSDoc it defaults to
`defaultImportability` (public in these tests, so no error); with JSDoc directly on
the `export * as ns from ...` statement, that JSDoc governs, the exporter file is
`barrel.ts` (where the statement lives — this is a genuine one-hop stop, unlike a
plain `export * from`, because `export * as ns` introduces an actual named
declaration/symbol in `barrel.ts`), and the reported identifier is the namespace
name (`ns`), not any name inside the namespace.

## Summary for `lookup()` (PLAN.md §2.3)

```
lookup(target_file, name):
    if name in target_file.export_table:
        # covers: local declarations, named re-exports (`export {x} from`),
        # `export * as ns from` (as its own name "ns"), export=/export default.
        # This is a genuine one-hop stop — even if the entry is itself a
        # passthrough re-export with no own JSDoc, resolution terminates here.
        return target_file, target_file.export_table[name]
    for star_target in target_file.star_exports:   # depth-first, cycle-guarded
        result = lookup(star_target, name)          # NOT one-hop: recurses fully
        if result is found:
            return result                            # exporter = the *terminal* file
    return not found
```

The key asymmetry to encode: named re-exports stop resolution at one hop (the
re-exporting file itself becomes "the exporter", governed by its own JSDoc or
lack thereof); star exports impose **no hop limit** and always resolve to the
terminal declaring file, both for JSDoc lookup and for the in-package directory
check. `export * as ns from` is *not* a star export for this purpose — it creates
its own named export table entry (`ns`) subject to ordinary one-hop rules.

## Caveats

- The "no JSDoc, `defaultImportability` = public" cases (Q1's `@package` sibling
  fixture, Q5 plain case) don't independently prove enforcement — they're
  consistent with either "checked but passes" or "not checked". Where this
  mattered, a directory-independent (`@private`) or directory-discriminating
  fixture was used instead (Q1's private variant, Q2a/Q2b, Q3's dircheck triangulation).
- Not tested: interaction of star-export flattening with `excludeSourcePatterns`
  or `treatSelfReferenceAs` — out of scope for this spike.
