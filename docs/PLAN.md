# ImportLint — npm Distribution Plan

ImportLint v0.1.0 shipped as a Rust CLI (crates.io: `import-lint` / `import-lint-core`;
prebuilt binaries on GitHub Releases via `.github/workflows/release.yml`). Its users,
however, live in the JS/TS ecosystem: they expect `npm install -D import-lint` and
`npx import-lint`, wired into `package.json` scripts and CI alongside their other
tooling. This plan adds first-class npm distribution.

The completed v1 implementation plan (M0–M7) is archived at
[`docs/PLAN-v1.md`](./PLAN-v1.md); this document only covers the npm packages.

**Goal:** `npm install -D import-lint && npx import-lint` works on every supported
platform with no compilation, no network access beyond the npm registry, and no
install scripts — and the npm version always corresponds exactly to the crates.io /
GitHub Release version.

---

## 1. Product decisions (locked)

| # | Decision | Rationale |
|---|---|---|
| P1 | Main npm package **`import-lint`** (verified unclaimed on the npm registry 2026-07-11); per-platform binary packages under a scoped org: **`@import-lint/linux-x64-gnu`**, **`@import-lint/linux-x64-musl`**, **`@import-lint/linux-arm64-gnu`**, **`@import-lint/darwin-x64`**, **`@import-lint/darwin-arm64`**, **`@import-lint/win32-x64`**. The `import-lint` npm org must be created once (runbook step) before first publish. | Matches the ecosystem convention (`@esbuild/*`, `@biomejs/cli-*`, `@rspack/binding-*`). A scope means six platform names can't be individually squatted and signals they're internal artifacts, not user-facing packages. Node-style `os-cpu-libc` naming (not Rust triples) because the resolving code speaks `process.platform`/`process.arch`. |
| P2 | Distribution mechanism: **`optionalDependencies` + JS launcher shim** — each platform package declares `os`/`cpu` (and `libc` where applicable) so package managers install exactly the matching one; the main package's `bin` is a small CommonJS shim that locates the platform package via `require.resolve` and `spawnSync`s the real binary. **No postinstall download scripts.** | The esbuild/Biome pattern. Postinstall downloaders break under `--ignore-scripts` (common in security-conscious setups), offline mirrors, and registry proxies; optionalDependencies ride the normal npm install/cache/lockfile path. |
| P3 | Supported targets = **exactly the six in `release.yml`**: `darwin-arm64`, `darwin-x64`, `linux-x64-gnu`, `linux-x64-musl`, `linux-arm64-gnu`, `win32-x64`. glibc-vs-musl on Linux is decided **in the shim at runtime** via `process.report.getReport().header.glibcVersionRuntime` (absent ⇒ musl), because the package-manager-side `libc` field is honored by pnpm and newer npm but not universally. Both Linux x64 packages carry the `libc` field anyway for managers that do honor it. | One source of truth for the target list (the release workflow). Runtime detection is what Biome/oxlint ship; it degrades gracefully where `libc` metadata is ignored (both variants installed, shim picks the right one). |
| P4 | **npm version ≡ crate version**, always released together from the same `v*` tag. Platform packages are **exact-pinned** (`"@import-lint/linux-x64-gnu": "1.2.3"`, no `^`) in the main package's `optionalDependencies`. | A version skew between shim and binary is undebuggable in the field. Exact pins mean a lockfile-free `npx import-lint@x.y.z` is fully deterministic. |
| P5 | Publishing happens **only from CI**, as a new job in the existing `release.yml` (after the binary build matrix), authenticated with an `NPM_TOKEN` automation-token secret and **npm provenance** (`npm publish --provenance`, `id-token: write`). Publish order: six platform packages first, then the main package. A platform-package publish that fails mid-sequence is retried/resumed by re-running the job — the assemble step and publishes are idempotent (`npm publish` of an already-published version is treated as success by an explicit already-published check, not `|| true`). | The main package must never be live while a platform package it pins is missing. Provenance gives users a supply-chain attestation for free. Manual local npm publishing is a footgun (six packages, ordering, credentials) — CI is the only path. |
| P6 | Package sources are **checked into the repo as real files** under `npm/` with version `0.0.0` placeholders; a zero-dependency Node script `npm/scripts/assemble.mjs` stamps the real version (from the tag) and copies each binary from the workflow artifacts into its package before publish. | Reviewable in PRs (no generated-at-publish-time package.json), diffable, and testable locally without CI. |
| P7 | **`engines.node: ">=18"`**; the shim is plain CommonJS, no dependencies, no build step. | Node 18 is the oldest maintained LTS; CJS avoids ESM interop edge cases in the one file where compatibility matters most. |
| P8 | Unsupported platforms and `--omit=optional` installs fail **at run time** with an actionable error (detected platform key, list of supported targets, `cargo install import-lint` and GitHub-Releases fallbacks) — never at install time. | Failing `npm install` for the whole project because one dev machine is exotic is hostile; the linter not running is discoverable exactly when relevant. |
| P9 | The first npm release is the **next version bump** (e.g. v0.1.1) published via the normal tag flow — no retroactive npm publish of v0.1.0. | The v0.1.0 tag's workflow run predates the npm job; re-tagging published artifacts is more error-prone than a patch release. |

## 2. Package layout

```
npm/
├── import-lint/                    # the package users install
│   ├── package.json                # name, bin, optionalDependencies (6, exact-pinned),
│   │                               # engines, repository, license: MIT, files: ["bin/"]
│   ├── README.md                   # npm-facing readme (quickstart + link to repo)
│   └── bin/
│       └── import-lint.js          # the launcher shim (CJS, zero deps, #!/usr/bin/env node)
├── platform/
│   ├── linux-x64-gnu/package.json  # { name: "@import-lint/linux-x64-gnu",
│   │                               #   os: ["linux"], cpu: ["x64"], libc: ["glibc"],
│   │                               #   files: ["import-lint"] }
│   ├── linux-x64-musl/…            # libc: ["musl"]
│   ├── linux-arm64-gnu/…           # os linux, cpu arm64, libc glibc
│   ├── darwin-x64/…                # os darwin, cpu x64
│   ├── darwin-arm64/…              # os darwin, cpu arm64
│   └── win32-x64/…                 # os win32, cpu x64; binary is import-lint.exe
└── scripts/
    ├── assemble.mjs                # stamp version + copy binaries from a dist/ dir
    └── smoke.mjs                   # pack + install + run in a temp dir (used locally & in CI)
```

Each platform package contains exactly its `package.json`, a one-line README, and (after
assembly) the binary at package root. The binary file name is `import-lint`
(`import-lint.exe` on Windows) — the shim resolves
`@import-lint/<key>/import-lint<ext>`.

## 3. The launcher shim

Behavior of `npm/import-lint/bin/import-lint.js`:

1. Compute the platform key: `process.platform`-`process.arch`, plus `-gnu`/`-musl` on
   Linux via the `process.report` glibc probe (fallback: `fs.existsSync`-probe for
   `/etc/alpine-release` ⇒ musl) .
2. Env override **`IMPORT_LINT_BINARY`**: if set, skip resolution and run that path
   (debugging aid, also used by the smoke script before publish).
3. `require.resolve("@import-lint/<key>/import-lint<ext>")` — on failure, print the P8
   error to stderr and exit 2 (matching the CLI's "usage/internal error" exit-code
   contract).
4. `child_process.spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" })`,
   then mirror the child: `process.exit(status)`; if terminated by a signal, re-raise
   it (`process.kill(process.pid, signal)`).

Notably absent: no update checks, no telemetry, no config — the shim is a dumb exec
forwarder and must stay that way (it runs on every lint invocation).

## 4. CI: extending `release.yml`

New jobs after the existing build matrix (which already uploads per-target artifacts):

1. **`npm-assemble`** (ubuntu): download all six binary artifacts → `node
   npm/scripts/assemble.mjs --version ${TAG#v} --dist <artifacts-dir>` → verify every
   package dir now contains its binary and the stamped version → upload the seven
   assembled package dirs as one artifact. Fails if any binary is missing (a
   half-release must be impossible).
2. **`npm-smoke`** (matrix: ubuntu, macos, windows): download assembled packages, `npm
   pack` each, then in a temp project `npm install` the main tarball plus the local
   platform tarballs (`overrides` pointing the scoped names at the `.tgz` files so no
   registry fetch occurs), run `npx import-lint --version` (assert tag version) and
   lint a two-file fixture with a `@package` violation (assert exit 1 and the expected
   diagnostic on stdout). This exercises the real shim → binary path on all three OSes
   pre-publish.
3. **`npm-publish`** (needs: assemble + smoke + the GitHub-release job; `environment`-
   gated if we want a manual approval click later): for each platform package then the
   main package, check the registry for name@version first (skip if present — makes
   re-runs safe), else `npm publish --provenance --access public`.

`RELEASING.md` gains two one-time steps (create the `import-lint` npm org; add the
`NPM_TOKEN` automation token as a repo secret) — the per-release runbook is unchanged:
pushing the `v*` tag now also ships npm.

## 5. Testing strategy

- **Shim unit-ish tests** (plain `node --test` in `npm/import-lint/test/`, run by CI's
  normal test job, no Rust needed): platform-key computation (mock `process` values),
  the P8 error path (unresolvable package ⇒ exit 2 + helpful stderr), env override.
- **`npm/scripts/smoke.mjs`** runs the full local loop on the developer's host target:
  `cargo build --release` → assemble just the host package with a dev version → pack →
  temp-dir install → `--version` + violation-fixture checks. Wired as the entry point
  the CI `npm-smoke` job shares, so local and CI runs can't drift.
- CI smoke on all three OS runners per release tag (see §4) is the release gate.

## 6. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| npm org name `import-lint` unavailable | Blocked scoped naming | Verify at org-creation time (first runbook step, before anything is published); fallback: `@importlint/*` scope — only the shim's resolve strings and package names change. |
| libc misdetection (containers, static Node, FreeBSD-linuxulator oddities) | Wrong/no binary picked | `process.report` probe + alpine fallback + `IMPORT_LINT_BINARY` escape hatch + P8's actionable error naming the computed key. |
| Package manager ignores `os`/`cpu`/`libc` and installs all six | Wasted disk, not broken | Accepted: shim picks correctly regardless. Document pnpm's `supportedArchitectures` for mono-arch installs. |
| `--omit=optional` / `--no-optional` installs | Shim has no binary | P8 runtime error explains exactly what happened and how to fix it. |
| Partial publish (some platform packages live, main missing or vice versa) | Broken installs for a version | Strict publish order (P5), idempotent re-runs, smoke gate before any publish. |
| Windows shim quirks (`spawnSync` + `.exe`, path spaces) | Broken on Windows | Direct `spawnSync` of the resolved absolute `.exe` path (no shell), covered by the windows leg of `npm-smoke`. |

## 7. Milestones

**N1 — Wrapper packages + shim (local):** `npm/` tree as in §2, shim per §3,
`assemble.mjs` + `smoke.mjs`, shim tests green, local smoke green on the dev machine
(linux-x64-gnu). Exit: `npm/scripts/smoke.mjs` passes locally end-to-end.

**N2 — CI publish pipeline:** §4 jobs in `release.yml` (assemble → 3-OS smoke →
publish with provenance + already-published skip), `RELEASING.md` one-time steps
documented, org + `NPM_TOKEN` created by the maintainer. Exit: a `v0.1.x` tag ships
crates.io (unchanged), GitHub Release binaries (unchanged), **and** the seven npm
packages, with `npx import-lint@0.1.x --version` working on all three OSes.

**N3 — Docs & adoption polish:** npm README (the package page is many users' first
contact), root README install section leads with npm, migration guide updated to
"replace `eslint-plugin-import-access` with `import-lint` in devDependencies",
CI-usage recipe (GitHub Actions step using `--format github`). Exit: docs reviewed,
v0.1.x announced.

## 8. Explicitly out of scope

- Native Node bindings (napi-rs) — the CLI boundary is sufficient until an
  LSP/editor-integration milestone (v1 plan's M8) demands in-process calls.
- Bun-/Deno-specific packages (both consume the npm packages as-is).
- Self-update, version checks, or any network activity in the shim.
- Publishing v0.1.0 retroactively to npm (P9).
