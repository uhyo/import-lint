# Concepts

ImportLint enforces directory-level encapsulation for TypeScript and
JavaScript: a directory is a "package", and in the recommended setup its
exports are importable only from inside it until tagged `@public` in their
JSDoc — ImportLint flags every import that crosses a boundary it shouldn't.
This guide defines the terms that model uses, each with a small example you
can reproduce yourself. It's written for someone who has never used
ImportLint or its ancestor,
[`eslint-plugin-import-access`](https://github.com/uhyo/eslint-plugin-import-access) —
if you already know that plugin, every option here maps 1:1 by name (see the
root README's [Migration](../../README.md#migration-from-eslint-plugin-import-access)
section).

If you'd rather learn by doing first, see [`tutorial.md`](./tutorial.md). If
you're choosing how to roll ImportLint out to a real project, see
[`adoption.md`](./adoption.md).

## Package

A **package**, in ImportLint's vocabulary, has nothing to do with an npm
package — it's the unit of encapsulation the tool checks. Packages-scoped
(or *package-private*) exports are importable only from inside the same package. Packages can be nested — child packages can import from parent packages, but
not the other way around.

By default, a file's package is its own containing directory: a package-scoped
export is importable from its own directory or from any subdirectory of that
directory, but not from a parent directory or a sibling directory.

**Example 1:**

```
src/
├── cart/
│   ├── total.ts        ── exports computeTotal
│   ├── checkout.ts     ── same directory: can import computeTotal
│   └── promo/
│       └── discount.ts  ── nested subdirectory: can ALSO import computeTotal
└── receipt.ts            ── parent directory: cannot
```

The recommended config setup is `"defaultImportability": "package"` — that is,
every export is package-private unless tagged `@public`. You can still use
`@package` tags to make an export package-private even in a `"public"`-default codebase. See [Importability](#importability) below.

`src/cart/total.ts`:

```ts
export function computeTotal(items: number[]): number {
  return items.reduce((a, b) => a + b, 0);
}
```

`src/cart/promo/discount.ts` — two directories below `total.ts`:

```ts
import { computeTotal } from "../total";

export function discountedTotal(items: number[]): number {
  return computeTotal(items) * 0.9;
}
```

`src/receipt.ts` — a sibling of `cart/`, not nested inside it:

```ts
import { computeTotal } from "./cart/total";

console.log(computeTotal([1, 2, 3]));
```

`src/cart/checkout.ts` (same directory) and `src/cart/promo/discount.ts`
(nested two levels down) both import `computeTotal` cleanly. `src/receipt.ts`
is outside `src/cart/` entirely — not the same directory, and not nested
inside it — so it's the only one that gets a diagnostic:

```
src/receipt.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  package-access

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
  same package. Useful for test-only exports by configuring ImportLint to ignore test files.

(`/** @access public */` / `@access package` / `@access private` are accepted
as an alternate spelling of the same three tags.)

An export with **no** recognized tag falls back to the `defaultImportability`
option (`"public"` | `"package"` | `"private"`) —
this is the single option that decides whether an *unannotated* codebase
starts wide open or fully closed.

**Example 2:**

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

With `defaultImportability` at its built-in default (`"public"`), this gets exactly
one diagnostic — `computeTotal` (public) and `untaggedHelper` (defaults to
public) are both fine, `internalRound` isn't:

```
src/receipt.ts
  1:24  error  Cannot import a private export 'internalRound'  package-access

✖ 1 problem (1 error, 0 warnings)
```

Flip `defaultImportability` to `"package"` in the config and the untagged
export becomes restricted too — now the same import statement produces two
diagnostics:

```
src/receipt.ts
  1:24  error  Cannot import a private export 'internalRound'  package-access
  1:39  error  Cannot import a package-private export 'untaggedHelper'  package-access

✖ 2 problems (2 errors, 0 warnings)
```

`defaultImportability: "package"` is what makes tagging *optional-by-default,
restrictive-by-default* — the recommended setting, and what `import-lint init`
scaffolds (see [`adoption.md`](./adoption.md)). The built-in default is
`"public"` for backward compatibility with `eslint-plugin-import-access`, which treated untagged exports as public.

## Package directory

By default a file's package is its own directory.
The `packageDirectory` option replaces that
with a set of glob patterns identifying which *ancestor* directories count
as package boundaries — every file under one of those directories, at any
depth, is in the same package, and nothing outside is (unless a *different*
`packageDirectory` boundary is nested inside the first one — see the note
at the end of [Index loophole](#index-loophole) below, where that
distinction actually matters).

Each pattern is matched against both a candidate directory's **basename**
and its **project-relative path**, so a pattern like `"**/*.package"` matches
by name regardless of where the directory lives. This is the `*.package`
naming convention the `import-lint init` config (see
[`adoption.md`](./adoption.md)) is built on: name any directory that should
be a boundary `foo.package`.

**Example 3:**

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
`tokens/` has no boundary of its own, so these two files are in the same package, `auth.package`. 

On the other hand, `src/checkout/pay.ts` importing `sign` from `../auth.package/tokens/sign` is a violation — `checkout/` is outside the `auth.package` boundary, so it can't reach in to a package-private export:

```ts
import { sign } from "../auth.package/tokens/sign";

export function pay() {
  return sign("order-1");
}
```

```
src/checkout/pay.ts
  1:10  error  Cannot import a package-private export 'sign'  package-access

✖ 1 problem (1 error, 0 warnings)
```

A `!`-prefixed pattern excludes a directory that would otherwise match — e.g.
`["**", "!**/scratch"]` makes every directory a boundary except ones named
`scratch`, whose files fall back to their *parent's* boundary instead.

A file with **no** matching ancestor at all belongs to one project-wide package rooted
at the project root: all such files import freely from each other (and from
inside them, boundary-dwelling files can still reach root-package exports, per
the nesting rule above), while matched boundaries stay sealed.

So with `["**/*.package"]` on a codebase with no `*.package` directories yet, nothing
is restricted — each directory you rename adds one enforced boundary, which is
what makes gradual adoption of the naming convention work.

## Index loophole

`indexLoophole` (default: **on**) treats a file named `index.{js,ts,jsx,tsx,mjs,cjs,mts,cts}`
(but not `index.d.ts`) as if its parent directory were the exporting file,
for package-boundary purposes. Concretely: an export that reaches the
outside world only via a re-export in a boundary's
`index.ts` gets promoted one level out — to the boundary's *parent's*
package — instead of staying trapped inside.

**Example 4:**

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

still can't; that deep path crosses the `auth.package` boundary:

```
src/checkout.ts
  1:10  error  Cannot import a package-private export 'secretKey'  package-access

✖ 1 problem (1 error, 0 warnings)
```

Only once `auth.package/index.ts` *also* adds its own bare
`export { secretKey } from "./secrets.package";` does the promotion reach
one level further out, to `src/` — and even then, only through
`./auth.package` (the outer index), not through a direct reach-in.

## Filename loophole

`filenameLoophole` (default: **off**) is the companion-file pattern: a file
`foo.ts` sitting next to a directory `foo/` is treated as in-package with
everything **directly** inside `foo/` (one level only — not
`foo/nested/bar.ts`).

**Example 5:**

```
src/
├── cart.ts       ── companion file
└── cart/
    └── total.ts  ── exports computeTotal, @package
```

Assuming all directories are package boundaries and
`filenameLoophole: false` (the default), `src/cart.ts` importing from
`./cart/total`:

```ts
import { computeTotal } from "./cart/total";

console.log(computeTotal([1, 2, 3]));
```

is a normal cross-package reach-in and fails:

```
src/cart.ts
  1:10  error  Cannot import a package-private export 'computeTotal'  package-access

✖ 1 problem (1 error, 0 warnings)
```

With `filenameLoophole: true`, the same import is clean — `cart.ts` and
`cart/` are treated as one package.

The `cart.ts` effectively stands on the `cart/` package boundary  —
as an importer it behaves like it's inside the `cart/` package, and as an exporter it behaves like it's outside the `cart/` package. This is useful for a "public API"
file that re-exports everything from a directory, while still being able to
reach into that directory for private helpers.

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

**Example 6:**

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
  1:10  error  Cannot import a package-private export 'computeTotal'  package-access

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
  1:10  error  Cannot re-export a package-private export 'computeTotal'  package-access

✖ 1 problem (1 error, 0 warnings)
```

## External vs. internal

ImportLint only ever checks **internal** imports — specifiers that resolve
to a file inside your project. An npm dependency or a Node.js builtin, etc. is
**external** and is never checked, regardless of what access tag the target
file's exports carry.

**Example 7:*

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

This lints clean — `left-pad`'s own `@private` tag is irrelevant, because the
import never resolves to an internal file.

**Self-references** — importing your own package by its `package.json`
`name` instead of a relative path — are a gray
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