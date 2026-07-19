# Tutorial: your first boundary

This is a hands-on, ~10-minute walkthrough: create one encapsulation
boundary, trigger a real violation, and fix it three different ways. It
assumes no knowledge about ImportLint or JSDoc access tags beyond what's
explained inline — for the concepts behind each step, see
[`concepts.md`](./concepts.md).

## Setup

You need Node.js (for `npx`) or a downloaded `import-lint` binary. Make an empty directory and step
into it:

```sh
mkdir import-lint-tutorial && cd import-lint-tutorial
```

Scaffold the config — the recommended package-by-default setup, built on the
`*.package` naming convention:

```sh
npx @import-lint/cli init
```

```
Wrote .importlintrc.jsonc
```

The generated file is fully commented — every option annotated in place. The
two options that drive this walkthrough:

```jsonc
// Convention: any directory named "foo.package" is an encapsulation boundary.
// Everything inside it imports freely from everything else inside it; nothing
// outside can import an export unless it's tagged `@public`.
{
  "rules": {
    "package-access": {
      // Every export is package-scoped by default (no JSDoc tag needed).
      "defaultImportability": "package",

      // A directory is a boundary because of its name, not its location.
      "packageDirectory": ["**/*.package"]
    }
  }
}
```

## Make a boundary

Create a `src/auth.package/` directory — its name makes it a boundary under
the config above — with one function inside it, and one file outside that
tries to use it:

```sh
mkdir -p src/auth.package
```

`src/auth.package/token.ts`:

```ts
export function issueToken(userId: string): string {
  return `token-for-${userId}`;
}
```

`src/server.ts`:

```ts
import { issueToken } from "./auth.package/token";

console.log(issueToken("alice"));
```

Your directory now looks like:

```
.
├── .importlintrc.jsonc
└── src
    ├── auth.package
    │   └── token.ts
    └── server.ts
```

## Hit a real error

```sh
npx @import-lint/cli .
```

```
src/server.ts
  1:10  error  Cannot import a package-private export 'issueToken'  package-access

✖ 1 problem (1 error, 0 warnings)
```

Exit code `1`. `issueToken` has no JSDoc tag, so it defaults to
`"package"` importability (that's what `defaultImportability` set); `token.ts`
lives inside the `auth.package` boundary, and `server.ts` doesn't, which marks the
import as a violation. This is the thing ImportLint exists to catch: a
function that was only ever meant for `auth.package`'s own internals, used
somewhere it shouldn't be.

There are three independent ways to fix this. Each one below assumes you're
starting back from the violating state above.

### Fix 1: tag the export `@public`

If `issueToken` really is meant to be used from anywhere, say so explicitly.
`src/auth.package/token.ts`:

```ts
/** @public */
export function issueToken(userId: string): string {
  return `token-for-${userId}`;
}
```

```sh
npx @import-lint/cli .
```

No output, exit code `0` — `pretty` format prints nothing on a clean run.

### Fix 2: re-export it through the boundary's `index.ts`

Revert the tag from Fix 1 first (delete the `/** @public */` line) — the
violation is back:

```
src/server.ts
  1:10  error  Cannot import a package-private export 'issueToken'  package-access

✖ 1 problem (1 error, 0 warnings)
```

Instead of tagging the function itself, add a bare re-export at the
boundary's own `index.ts`, and import through it instead of reaching
straight into `token.ts`.

`src/auth.package/index.ts`:

```ts
export { issueToken } from "./token";
```

`src/server.ts`, updated to import through the index:

```ts
import { issueToken } from "./auth.package";

console.log(issueToken("alice"));
```

```sh
npx @import-lint/cli .
```

No output, exit code `0`. This works because of the *index loophole*
(`concepts.md`'s [Index loophole](./concepts.md#index-loophole) section): a
bare re-export in a boundary's `index.ts` promotes that export to the
boundary's parent package — `src/`, the same directory `server.ts` is in.
Unlike Fix 1, `issueToken` is not *fully* public — it is still inaccessible to anything two levels
away.

### Fix 3: move the importer inside the boundary

Revert Fix 2 (delete `src/auth.package/index.ts`, and change `server.ts`'s
import back to `./auth.package/token`) — the violation returns. Instead of
changing what's exported, move the file that needs it into the boundary:

```sh
mv src/server.ts src/auth.package/server.ts
```

`src/auth.package/server.ts`, with the import path updated to match its new
location:

```ts
import { issueToken } from "./token";

console.log(issueToken("alice"));
```

```sh
npx @import-lint/cli .
```

No output, exit code `0`. `server.ts` is now in the same package as
`token.ts`, so no tag or re-export is needed at all.

## Conclusion

In this tutorial, you've created a boundary, hit a real violation,
and seen three independent, valid ways to resolve it — which one is
"correct" depends entirely on whether the export was meant to be public API,
a curated re-export surface, or purely internal to code that should just
live inside the boundary.

However, `@public` is rarely the right choice; it makes the export available *everywhere*, which is usually not what you want. The other two fixes are more common: either re-export it through the boundary's `index.ts` so that the boundary can control its public API, or move the importer into the boundary so it can use the export without exposing it to the outside world.

## Where next

- [`concepts.md`](./concepts.md) — the full mental model: package
  directories, both loopholes and their cascade behavior, re-export
  semantics, and what counts as external vs. internal.
- [`adoption.md`](./adoption.md) — choosing a starting configuration
  (package-by-default, annotation-driven, monorepo) for a real project, and a
  phased rollout strategy for retrofitting an existing codebase.
