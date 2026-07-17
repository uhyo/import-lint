# Concepts

Documents ImportLint v0.1.2.

ImportLint enforces directory-level encapsulation for TypeScript and
JavaScript: it reads JSDoc access tags on your `export` statements and flags
imports that cross a boundary they shouldn't. This guide defines the terms
that model uses, each with a small example you can reproduce yourself. It's
written for someone who has never used ImportLint or its ancestor,
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access) —
if you already know that plugin, every option here maps 1:1 by name (see the
root README's [Migration](../../README.md#migration-from-eslint-plugin-import-access)
section).

If you'd rather learn by doing first, see [`tutorial.md`](./tutorial.md). If
you're choosing how to roll ImportLint out to a real project, see
[`adoption.md`](./adoption.md).

Every example below was actually run against the workspace binary; the code
shown is the exact file content that produced the diagnostic underneath it
(no filename comment baked in — where a snippet's file matters, it's named
in the sentence right above the code block).

## Package

A **package**, in ImportLint's vocabulary, has nothing to do with an npm
package — it's the unit of encapsulation the tool checks: by default, a
package is a directory. Every file in that directory (not its
subdirectories, unless a loophole applies — see below) is "in the same
package". An export tagged for package-only visibility can be imported by
any file in its own package, and by nothing else.

```
src/
├── cart/
│   ├── total.ts     ── exports computeTotal, tagged @package
│   └── checkout.ts  ── same directory: can import computeTotal
└── receipt.ts       ── different directory: cannot
```

`src/cart/total.ts`:

```ts
/** @package */
export function computeTotal(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}
```

`src/receipt.ts`:

```ts
import { computeTotal } from "./cart/total";

console.log(computeTotal([1, 2, 3]));
```

`src/cart/checkout.ts` importing `computeTotal` from `./total` is fine — same
directory, same package. `src/receipt.ts` is outside `src/cart/`, so it gets
a real diagnostic:

```
src/receipt.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

Where a file's package boundary actually sits — directory-by-default, or
something else — is configurable; see [Package directory](#package-directory)
below.

## Importability

**Importability** is the access level ImportLint resolves for an export,
before checking whether an importer is allowed to see it. There are three
levels, declared with a JSDoc tag directly above the `export`:

- `@public` — importable from anywhere.
- `@package` — importable only from within the same package (see
  [Package](#package) above).
- `@private` — not importable from anywhere, not even from files in the
  same package. (The export is only usable inside its own file — verified:
  a same-directory import of a `@private` export is rejected too.)

(`/** @access public */` / `@access package` / `@access private` are accepted
as an alternate spelling of the same three tags.)

An export with **no** recognized tag falls back to the `defaultImportability`
option (`"public"` | `"package"` | `"private"`, default `"public"`) — this is
the single option that decides whether an *unannotated* codebase starts wide
open or fully closed.

`src/cart/total.ts`:

```ts
/** @public */
export function computeTotal(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}

/** @private */
export function internalRound(n: number): number {
  return Math.round(n);
}

export function untaggedHelper(n: number): number {
  return n;
}
```

`src/receipt.ts`, outside `src/cart/`:

```ts
import { computeTotal, internalRound, untaggedHelper } from "./cart/total";

console.log(computeTotal([1, 2, 3]), internalRound(1.4), untaggedHelper(2));
```

With `defaultImportability` at its default (`"public"`), this gets exactly
one diagnostic — `computeTotal` (public) and `untaggedHelper` (defaults to
public) are both fine, `internalRound` isn't:

```
src/receipt.ts
  1:24  error  Cannot import a private export 'internalRound'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

Flip `defaultImportability` to `"package"` in the config and the untagged
export becomes restricted too — now the same import statement produces two
diagnostics:

```
src/receipt.ts
  1:24  error  Cannot import a private export 'internalRound'  import-access/jsdoc
  1:39  error  Cannot import a package-private export 'untaggedHelper'  import-access/jsdoc

✖ 2 problems (2 errors, 0 warnings)
```

`defaultImportability: "package"` is what makes tagging *optional-by-default,
restrictive-by-default* — the `standard` preset in
[`adoption.md`](./adoption.md) uses it.

## Package directory

By default a file's package is simply its containing directory (as shown
above). The `packageDirectory` option replaces that with a set of glob
patterns identifying which *ancestor* directories count as package
boundaries — every file under one of those directories, at any depth, is in
the same package, and nothing outside is.

Each pattern is matched against both a candidate directory's **basename**
and its **project-relative path**, so a pattern like `"**/*.package"` matches
by name regardless of where the directory lives. This is the `*.package`
naming convention the `standard` preset (see [`adoption.md`](./adoption.md))
is built on: name any directory that should be a boundary `foo.package`.

```
src/
├── auth.package/
│   ├── tokens/
│   │   └── sign.ts     ── exports sign, untagged (defaultImportability: package)
│   └── session.ts      ── nested arbitrarily deep, still same package
└── checkout/
    └── pay.ts           ── outside the boundary
```

Config: `"packageDirectory": ["**/*.package"]`, `"defaultImportability": "package"`.

`src/auth.package/session.ts` importing `sign` from `./tokens/sign` is fine —
`tokens/` has no boundary of its own, so it inherits `auth.package/`'s.
`src/checkout/pay.ts`, reaching in from outside:

```ts
import { sign } from "../auth.package/tokens/sign";

export function pay() {
  return sign("order-1");
}
```

```
src/checkout/pay.ts
  1:10  error  Cannot import a package-private export 'sign'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

A `!`-prefixed pattern excludes a directory that would otherwise match — e.g.
`["**", "!**/scratch"]` makes every directory a boundary except ones named
`scratch`, whose files fall back to their *parent's* boundary instead. A file
outside every matching boundary (and not excluded into one) falls back to
its own containing directory, same as the no-`packageDirectory` default.

## Index loophole

`indexLoophole` (default: **on**) treats a file named `index.{js,ts,jsx,tsx,mjs,cjs,mts,cts}`
(but not `index.d.ts`) as if its parent directory were the exporting file,
for package-boundary purposes. Concretely: an export that reaches the
outside world only via a **bare** (untagged) re-export in a boundary's
`index.ts` gets promoted one level out — to the boundary's *parent's*
package — instead of staying trapped inside.

```
src/
├── auth.package/
│   ├── sign.ts    ── exports sign, untagged
│   └── index.ts   ── export { sign } from "./sign";  (bare re-export)
└── checkout.ts    ── same directory as auth.package's parent (src/)
```

With `packageDirectory: ["**/*.package"]` and `defaultImportability: "package"`,
`src/checkout.ts` importing `sign` straight from `./sign.ts` would fail (it's
outside the boundary) — but importing it from `./auth.package` (the index)
succeeds, because the index loophole treats `auth.package/index.ts` as
belonging to `auth.package`'s *parent* directory, `src/`, the same package
`checkout.ts` is in:

```
$ import-lint .
(no output — clean)
```

**Promotion cascades one level at a time, not all the way out.** Tag the
re-export itself and its own JSDoc governs instead of promoting — a
`/** @private */` on that same `export { sign } from "./sign"` line makes the
import fail again, this time with a private-export diagnostic, because the
re-export's own tag is what's consulted (see
[Re-exports and one-hop semantics](#re-exports-and-one-hop-semantics) below).

The cascade is visible with a nested boundary. Given

```
src/
└── auth.package/
    ├── secrets.package/
    │   ├── key.ts     ── exports secretKey, @package
    │   └── index.ts   ── export { secretKey } from "./key";  (bare)
    └── session.ts      ── in auth.package, not secrets.package
```

`secrets.package/index.ts`'s bare re-export promotes `secretKey` only as far
as `secrets.package`'s own parent boundary — `auth.package`. So
`session.ts` (inside `auth.package`) can import it from `./secrets.package`,
but `src/checkout.ts` (outside `auth.package` entirely), reaching straight
in:

```ts
import { secretKey } from "./auth.package/secrets.package";

console.log(secretKey);
```

still can't — that deep path skips the outer index and hits
`secrets.package`'s own table directly:

```
src/checkout.ts
  1:10  error  Cannot import a package-private export 'secretKey'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

Only once `auth.package/index.ts` *also* adds its own bare
`export { secretKey } from "./secrets.package";` does the promotion reach
one level further out, to `src/` — and even then, only through
`./auth.package` (the outer index), not through a direct reach-in. Every
step of wider exposure is a separate, visible edit to an `index.ts` file —
"cascades one deliberate level at a time" means exactly this: nobody can
widen visibility by two levels with one line.

## Filename loophole

`filenameLoophole` (default: **off**) is the companion-file pattern: a file
`foo.ts` sitting next to a directory `foo/` is treated as in-package with
everything **directly** inside `foo/` (one level only — not
`foo/nested/bar.ts`).

```
src/
├── cart.ts       ── companion file
└── cart/
    └── total.ts  ── exports computeTotal, @package
```

With `filenameLoophole: false` (the default), `src/cart.ts` importing from
`./cart/total`:

```ts
import { computeTotal } from "./cart/total";

console.log(computeTotal([1, 2, 3]));
```

is a normal cross-package reach-in and fails:

```
src/cart.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

With `filenameLoophole: true`, the same import is clean — `cart.ts` and
`cart/` are treated as one package.

## Re-exports and one-hop semantics

Re-export checking is **one-hop**: when a file re-exports a name
(`export { x } from "./y"` or `export * from "./y"`), only that re-export
statement's *own* JSDoc — or, for `export *`, the chain of star-exports it
falls through to find the name — governs whether a downstream importer can
see it. ImportLint never looks a second hop further, at what `./y` re-exports
*from*.

Two consequences fall out of this:

1. **A bare (untagged) re-export resets importability to
   `defaultImportability`** — even if the original export was `@public`.
   Visibility doesn't inherit through a re-export by default; it has to be
   restated.
2. **A tagged re-export's own tag wins**, restoring (or changing) visibility
   for whoever imports through it.

`src/cart/total.ts`:

```ts
/** @public */
export function computeTotal(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}
```

`src/cart/pub.ts` — a bare re-export, no JSDoc, even though the original is
`@public`:

```ts
export { computeTotal } from "./total";
```

With `defaultImportability: "package"`, `src/receipt.ts` importing
`computeTotal` from `./cart/pub` (not `./total` directly):

```ts
import { computeTotal } from "./cart/pub";

console.log(computeTotal([1, 2, 3]));
```

fails — the bare re-export reset it to package-private:

```
src/receipt.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

Tag `pub.ts`'s re-export line itself `/** @public */` and the same import
becomes clean again — the re-export's own tag governs.

The re-export **statement itself** is also checked, against the file it
re-exports from — a file outside `src/cart/`'s package can't even write a
bare re-export of a package-private export. `src/other/reexport.ts`:

```ts
export { computeTotal } from "../cart/total";
```

```
src/other/reexport.ts
  1:10  error  Cannot re-export a package-private export 'computeTotal'  import-access/jsdoc

✖ 1 problem (1 error, 0 warnings)
```

(Note the message: "re-export", not "import" — same rule, a different
diagnostic wording for a re-export statement vs. a plain import.)

## External vs. internal

ImportLint only ever checks **internal** imports — specifiers that resolve
to a file inside your project. Anything that resolves through
`node_modules` (a real npm dependency, a Node.js builtin, or — depending on
`treatSelfReferenceAs`, see below — your own package imported by name) is
**external** and is never checked, regardless of what access tag the target
file's exports carry.

`node_modules/left-pad/index.js`:

```ts
/** @private */
module.exports.pad = function pad(s) { return s; };
```

`src/main.ts`:

```ts
import { pad } from "left-pad"; // external: resolves through node_modules
console.log(pad("x"));
```

This lints clean (exit code `0`) even with `defaultImportability: "package"`
project-wide — `left-pad`'s own `@private` tag is irrelevant, because the
import never resolves to an internal file.

**Self-references** — importing your own package by its `package.json`
`name` (or an `exports` map subpath) instead of a relative path — are a gray
area, controlled by `treatSelfReferenceAs` (default: `"external"`). With a
`package.json` `name` of `"shop"` and an `exports` map entry
`"./cart": "./src/cart/total.js"`, `src/receipt.ts` can reach `computeTotal`
by name instead of by relative path:

```ts
import { computeTotal } from "shop/cart";
```

With `treatSelfReferenceAs: "external"` (the default), this is exempt, same
as any other node_modules-style resolution. With `treatSelfReferenceAs:
"internal"`, it's checked exactly like a relative import — if `computeTotal`
is package-private and `receipt.ts` is outside its package, it's a real
violation.

This same mechanic is the core of monorepo-style boundaries: a **name-based**
import of a sibling workspace package (`import { x } from "@proj/bar"`)
resolves through `node_modules` and is external, while a **relative**
reach-in across the same two packages (`import { x } from "../../bar/src/internal"`)
is internal and fully checked. See the `monorepo` preset in
[`adoption.md`](./adoption.md) for a worked example.
