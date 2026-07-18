# Adoption

Documents ImportLint v0.1.5.

ImportLint's `package-access` rule enforces directory-level encapsulation:
in the recommended setup, exports stay inside their "package" unless tagged
`@public` — or, in the opt-in mode, stay public until tagged
`@package`/`@private` — see [`concepts.md`](./concepts.md) for the full
mental model, or [`tutorial.md`](./tutorial.md) for a hands-on walkthrough of
one boundary end to end. This guide is about the next question: which
starting configuration fits your project, and how do you actually roll it
out.

`import-lint init` scaffolds one fully commented config — the recommended
**package-by-default** setup. The two other setups below differ from it only
in the starting values of two options — `defaultImportability` and
`packageDirectory` — everything else is the same fully editable config file
(see the root README's [Config file](../../README.md#config-file) section for
every option). Every command and config shown below is real, verified against
the workspace binary.

## Choosing a starting configuration

| Setup | What's a boundary | What's restricted by default | Best for |
|---|---|---|---|
| **Package-by-default** (what `init` scaffolds) | Any directory named `foo.package` (`packageDirectory: ["**/*.package"]`) | Everything inside a boundary — tag `@public` to expose an export outside it (`defaultImportability: "package"`) | The recommended default, for new *and* existing codebases: the boundary is visible in the file tree itself, never needs config updates as the project grows, and restricts nothing until you rename a directory (see the playbook below). |
| **Annotation-driven** | Every plain directory (no `packageDirectory` set) | Nothing — every export is public until you tag it (`defaultImportability: "public"`, every other option at its default) | Enforcing your *existing* directory structure without renaming anything: seal off one export at a time with `@package` tags. |
| **Monorepo** | Each `packages/*` workspace package (`packageDirectory: ["packages/*"]`) | Everything inside a package (`defaultImportability: "package"`) — but only for *relative* reach-ins; a sibling package imported *by name* is exempt | Monorepos where the workspace package, not the directory, is the real unit of ownership. |

Scaffold the first one:

```sh
import-lint init
```

```
Wrote .importlintrc.jsonc
```

For the other two, start from the same generated file and edit the two
options — each playbook below shows the exact distinguishing config.

## Playbook: package-by-default

Convention: any directory named `foo.package`, anywhere in the project, is a
boundary — `src/auth.package/`, `src/billing.package/reporting/` (nested
depth doesn't matter, only the boundary directory's own name does). The
distinguishing config (exactly what `import-lint init` scaffolds):

```jsonc
"defaultImportability": "package",
"packageDirectory": ["**/*.package"],
```

Because the boundary is named, not located, this scales with the project
with zero config edits: create `src/whatever.package/`, and it's
automatically a boundary. `tutorial.md` walks through creating one boundary,
hitting a real violation, and fixing it three ways — that walkthrough *is*
this setup's day-to-day workflow.

It's also safe to adopt incrementally on an existing codebase: files outside
every `*.package` directory all belong to one project-root package, so with no
`*.package` directories yet this config restricts nothing — each directory you
rename to `foo.package/` seals off exactly that one boundary, and the rest of
the codebase keeps compiling (well, linting) clean.

Two variants of the same idea, if `*.package` naming doesn't fit your
project (swap into `packageDirectory` instead of `["**/*.package"]`):

- **Inverse naming** — every directory is a boundary *except* ones opting out
  by name: `["**", "!**/*.internal"]`. A directory named `utils.internal/`
  isn't its own boundary; its files fall back to their parent directory's
  boundary instead — verified: a sibling directly in the parent can import
  from it, but a file elsewhere in the project cannot reach in.
- **Fixed location** — boundaries live under one top-level directory instead
  of being named by suffix: `["src/packages/*"]`. Patterns match against a
  directory's project-relative path, not just its basename, so this only
  matches directories that are literally one level under `src/packages/`.

## Playbook: annotation-driven

Maybe you want to enforce the directory structure you already have — no
`*.package` renames — and the boundaries you care about don't share a naming
pattern or a fixed location. Flipping `defaultImportability` to `"package"`
with every plain directory as a boundary would flag every cross-directory
import project-wide on day one — usually hundreds of diagnostics, and no way
to land the tool without a mass-annotation PR first. The annotation-driven
setup avoids that: every option is at its default
(`defaultImportability: "public"`, no `packageDirectory`), so installing it
is a no-op — nothing is restricted until you say so.

```jsonc
// distinguishing config: none. Every option is the built-in default.
{
  "rules": {
    "package-access": {
      "severity": "error"
    }
  }
}
```

To get here from the `init`-generated file: change `defaultImportability` to
`"public"` and delete the `packageDirectory` line (or just write the config
above).

The phasing strategy has four steps, and you can stop after any of them and
resume later — nothing forces you to finish in one pass.

**1. Install at all-defaults.** Commit the config above, run `import-lint`
in CI (see step 3) if you like — with nothing tagged yet, it's clean:

```sh
import-lint . --format github
```

```
(no output — clean, exit code 0)
```

**2. Annotate one high-value directory.** Pick a directory whose internals
keep leaking into the rest of the codebase — here, `src/billing/`'s
`computeInvoice`, used from an existing `src/report.ts`:

`src/billing/invoice.ts`:

```ts
/** @package */
export function computeInvoice(cents: number): number {
  return cents;
}
```

`src/report.ts` (pre-existing, unchanged):

```ts
import { computeInvoice } from "./billing/invoice";

console.log(computeInvoice(100));
```

The existing caller now shows up as a real diagnostic:

```sh
import-lint . --format github
```

```
::error file=src/report.ts,line=1,col=10,endLine=1,endColumn=24::Cannot import a package-private export 'computeInvoice'
```

Fix each one on its own terms — using the same three options from
`tutorial.md` (tag `@public` if it's genuinely meant to be shared, re-export
it through an `index.ts` if only a curated subset should be, or move the
caller inside if it really belongs there):

```sh
mv src/report.ts src/billing/report.ts
```

`src/billing/report.ts`, with the import path updated to match its new
location:

```ts
import { computeInvoice } from "./invoice";

console.log(computeInvoice(100));
```

```sh
import-lint . --format github
```

```
(no output — clean, exit code 0)
```

**3. Wire `--format github` into CI.** `--format github` emits [GitHub
Actions workflow commands](https://docs.github.com/en/actions/using-workflows/workflow-commands-for-github-actions)
so a new violation shows up as an inline PR annotation, right on the line
that broke the boundary — see the root README's
[Using in CI](../../README.md#using-in-ci) section for the full workflow
snippet. This is what makes annotating gradually *safe*: once
`src/billing/` is tagged, any new outside reach-in fails the PR check
immediately, instead of silently regressing until the next manual audit.

**4. Ratchet outward.** Repeat step 2 for the next directory whenever it's
convenient — a refactor that touches it, a bug caused by an unintended
cross-boundary dependency, or just working top-down through the most
leaky-looking parts of the tree first. There's no "finish line" enforced by
the tool; the point of this setup is that partial coverage is a fully valid,
fully enforced state; you're never running with anything less than everything
you've tagged so far. Once most of the codebase is tagged, consider
switching `defaultImportability` to `"package"` — the recommended end state,
converting to the package-by-default posture that `import-lint init`
scaffolds — so newly added files are covered automatically instead of
needing an explicit tag.

## Playbook: monorepo

Boundaries are workspace packages under `packages/*`, not directories in
general — the distinguishing config (from the `init`-generated file, replace
`packageDirectory`'s value):

```jsonc
"defaultImportability": "package",
"packageDirectory": ["packages/*"],
```

Add `apps/*` (or wherever else your workspace defines packages) to the
`packageDirectory` list if your workspace layout has more than one root.

The mechanic that makes this useful: a **relative** reach-in across
workspace packages is checked like any other cross-package import, but a
**name-based** import of a sibling package (resolved through
`node_modules`, the way npm/pnpm/Yarn workspaces link siblings) is
*external* and exempt — same as any other npm dependency (see
[`concepts.md`](./concepts.md#external-vs-internal)). Given

```
packages/
├── bar/
│   ├── package.json    ── "name": "@proj/bar"
│   └── src/
│       ├── index.ts     ── export { greet } from "./internal";  (bare)
│       └── internal.ts  ── exports greet, @package
└── foo/
    ├── package.json    ── "name": "@proj/foo"
    └── src/
        └── index.ts
```

`packages/foo/src/index.ts`:

```ts
// name-based: resolves through node_modules -> external, never checked
import { greet } from "@proj/bar";

// relative reach-in across the workspace boundary: internal, checked
import { greet as greetDirect } from "../../bar/src/internal";

console.log(greet("a"), greetDirect("b"));
```

```sh
import-lint .
```

```
packages/foo/src/index.ts
  5:10  error  Cannot import a package-private export 'greet'  package-access

✖ 1 problem (1 error, 0 warnings)
```

Only the relative import is flagged — the name-based import through
`@proj/bar`'s own public entry point (`packages/bar/src/index.ts`, which
re-exports `greet` without a tag, resetting it to that package's own
`defaultImportability`) is exempt regardless of what `greet` is tagged
inside `packages/bar`. This is the enforcement you actually want in a
monorepo: **use each package's published entry point**, never its
internals — the same discipline you'd already want from an external
dependency, now enforced against your own workspace siblings too.

`treatSelfReferenceAs` (default: `"external"`) governs one edge case this
doesn't cover by itself: a package importing *its own* name-based path
(`import { x } from "@proj/foo"` from inside `packages/foo`) is exempt by
default too, same as any other name-based resolution. Set it to `"internal"`
if you want a package's own name-based self-imports checked the same as a
relative one.
