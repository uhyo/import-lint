# Adoption

ImportLint's `package-access` rule enforces directory-level encapsulation:
in the recommended setup, exports stay inside their "package" unless tagged
`@public` — or, in the opt-in mode, stay public until tagged
`@package`/`@private` — see [`concepts.md`](./concepts.md) for the full
mental model, or [`tutorial.md`](./tutorial.md) for a hands-on walkthrough of
one boundary end to end. This guide is about the next question: how do you
roll ImportLint out on a real codebase.

The short answer: you don't need a migration at all. The config that
`import-lint init` scaffolds is gradually adoptable by construction — on day
one it restricts nothing, and each boundary you add seals off exactly one
directory while the rest of the codebase keeps linting clean. This guide
walks that rollout step by step, then covers two variations (annotation-only
and package-by-feature) that are a config edit away from the same generated file.

## The starting point

`import-lint init` scaffolds one fully commented config file,
`.importlintrc.jsonc` — every option annotated in place (see the root
README's [Config file](../../README.md#config-file) section for every
option). The two options that define the setup:

```jsonc
"defaultImportability": "package",
"packageDirectory": ["**/*.package"],
```

This represents the recommended convention: any directory named `foo.package`, anywhere in the project, is a
boundary — e.g. `src/auth.package/`, or `src/billing.package/services/reporting.package/` (reporting package nested inside the billing package).
Everything inside a boundary imports freely from everything else inside it;
nothing outside can import an export unless it's tagged `@public` or
exported through the boundary's own `index.ts`.

Two properties of this setup do the work in the rollout below:

- **Day one is a no-op.** Files outside every `*.package` directory all
  belong to one project-root package and import freely from each other. With
  no `*.package` directories yet, nothing is restricted — installing the
  config on an existing codebase produces zero diagnostics.
- **The boundary is named, not located.** A directory is a boundary because
  of its name, so adopting one more boundary is a directory rename, not a
  config edit — and the config never needs updating as the project grows.

## Rolling it out, one boundary at a time

**1. Scaffold and install.** In the project root:

```sh
import-lint init
```

```
Wrote .importlintrc.jsonc
```

Run it — with no `*.package` directories yet, it's clean on any codebase:

```sh
import-lint .
```

```
(no output — clean, exit code 0)
```

Commit the config. ImportLint is now installed and enforcing everything
you've sealed so far — which is nothing, and that's the point.

**2. Seal one boundary.** Pick a directory whose internals keep leaking into
the rest of the codebase — here, `src/billing/`, whose `computeInvoice` is
used from an existing `src/report.ts`:

`src/billing/invoice.ts`:

```ts
export function computeInvoice(cents: number): number {
  return cents;
}
```

`src/report.ts`:

```ts
import { computeInvoice } from "./billing/invoice";

console.log(computeInvoice(100));
```

Rename the directory to `src/billing.package/`, updating import paths to
match (your editor's rename refactoring does both at once):

```sh
mv src/billing src/billing.package
```

`src/report.ts`, with the import path updated:

```ts
import { computeInvoice } from "./billing.package/invoice";

console.log(computeInvoice(100));
```

Every existing outside reach-in now shows up as a diagnostic:

```sh
import-lint .
```

```
src/report.ts
  1:10  error  Cannot import a package-private export 'computeInvoice'  package-access

✖ 1 problem (1 error, 0 warnings)
```

Fix each one on its own terms — the same three options `tutorial.md` walks
through: tag the export `@public` if it's meant to be shared everywhere,
re-export it through the boundary's `index.ts` if only a curated subset
should be, or move the caller inside the boundary if it really belongs
there. Here `computeInvoice` is part of billing's intended surface, so the
curated `index.ts` fits:

`src/billing.package/index.ts`:

```ts
export { computeInvoice } from "./invoice";
```

`src/report.ts`, importing through the boundary's entry point:

```ts
import { computeInvoice } from "./billing.package";

console.log(computeInvoice(100));
```

```sh
import-lint .
```

```
(no output — clean, exit code 0)
```

One directory sealed; everything else is untouched and still lints clean.

**3. Wire `--format github` into CI.** `--format github` emits [GitHub
Actions workflow commands](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands)
so a new violation shows up as an inline PR annotation, right on the line
that broke the boundary — see the root README's
[Using in CI](../../README.md#using-in-ci) section for the full workflow
snippet.

**4. Ratchet outward.** Repeat step 2 for the next directory whenever it's
convenient — a refactor that touches it, a bug caused by an unintended
cross-boundary dependency, or just working top-down through the most
leaky-looking parts of the tree first.

## If `*.package` naming doesn't fit

What convention your team uses for boundaries is up to you — change the `packageDirectory` option as needed.

The core idea is that `packageDirectory` is a **meta-config** — it defines
the naming rule that identifies boundaries. The meta-config is not something
you change every time you add a new boundary; it's something you set once to
define the naming convention your team uses.

As a result, what directories are boundaries can be decided by the
owners of those directories through the naming, not by a central config file.

Below are some other naming conventions you could use instead of `*.package`:

- **Inverse naming** — `"packageDirectory": ["**", "!**/*.internal"]`: every directory is a
  boundary *except* ones opting out by name. A directory named
  `utils.internal/` isn't its own boundary; its files fall back to their
  parent directory's boundary instead. Note this variant is *not* a day-one no-op: with every plain
  directory a boundary, existing cross-directory imports are flagged
  immediately, so it fits new projects better than gradual adoption.
- **Fixed location** — `["src/packages/*"]`: boundaries live under one
  top-level directory instead of being named by suffix. Patterns match
  against a directory's project-relative path, not just its basename, so
  this only matches directories that are literally one level under
  `src/packages/`. Suited for codebases that exercise the "package-by-feature" pattern, where each feature is a workspace package in its own right; this setting helps enforce that no cross-feature imports sneak in. 

## Variation: annotation-driven

Maybe renaming directories is off the table — the boundaries you care about
don't share a naming pattern or a fixed location, and you want to enforce
the directory structure you already have. The annotation-driven setup keeps
the same gradual rollout but changes the adoption unit from *one directory
rename* to *one JSDoc tag*: every export is public until you tag it
`@package`. From the `init`-generated file, change `defaultImportability` to
`"public"` and delete the `packageDirectory` line — which leaves every
option at its built-in default:

```jsonc
{
  "rules": {
    "package-access": {
      "severity": "error"
    }
  }
}
```

Installing this is the same day-one no-op — nothing is restricted until you
say so. Instead of renaming `src/billing/` in step 2, tag one export:

`src/billing/invoice.ts`:

```ts
/** @package */
export function computeInvoice(cents: number): number {
  return cents;
}
```

With no `packageDirectory` set, every plain directory is its own boundary,
so the tag seals `computeInvoice` inside `src/billing/` — and the existing
caller in `src/report.ts` surfaces exactly like in step 2:

```sh
import-lint . --format github
```

```
::error file=src/report.ts,line=1,col=10,endLine=1,endColumn=24::Cannot import a package-private export 'computeInvoice'
```

Everything else in the playbook carries over unchanged: the same three
fixes, the same CI wiring, the same ratchet — tag by tag instead of rename
by rename. Once most of the codebase is tagged, consider switching
`defaultImportability` to `"package"` — converting to the package-by-default
posture that `import-lint init` scaffolds — so newly added files are covered
automatically instead of needing an explicit tag.