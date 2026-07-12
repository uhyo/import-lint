# Releasing

ImportLint ships two crates to crates.io — `import-lint-core` (library) and
`import-lint` (the CLI binary, which depends on `import-lint-core` by version, not
just by path) — plus prebuilt binaries attached to a GitHub Release, built by
`.github/workflows/release.yml` on push of a `v*` tag. `crates/gen-fixture` is a
dev-only fixture generator and is never published (`publish = false`).

The same tag push also assembles, smoke-tests (on ubuntu/macos/windows), and
publishes the seven npm packages (`import-lint` plus the six `@import-lint/*`
platform packages, docs/PLAN.md §2) with provenance — see "npm distribution
(one-time setup)" below for the setup this requires before the first npm release.

## Runbook (v0.1.0)

Run these in order, from a clean `master` with the version already set in
`Cargo.toml`'s `[workspace.package] version`:

```sh
git push origin master

cargo login            # once, if you haven't already authenticated this machine

cargo publish -p import-lint-core
# Wait for import-lint-core to appear on crates.io (usually well under a minute,
# but the index can lag) before publishing the CLI — it depends on the published
# version, not the local path, once packaged.
cargo publish -p import-lint

git tag v0.1.0 && git push origin v0.1.0   # triggers .github/workflows/release.yml,
                                            # which builds binaries for all targets,
                                            # creates the GitHub Release, and
                                            # assembles/smoke-tests/publishes npm
```

Sanity-check before publishing (both should already be clean on `master`, but
re-verify after any last-minute change):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo publish --dry-run -p import-lint-core
```

`cargo publish --dry-run -p import-lint` cannot fully succeed until
`import-lint-core` is live on crates.io (dependency resolution needs the published
version), so there's no reliable dry run for the CLI crate beyond
`cargo package -p import-lint --no-verify --list` — treat the real
`cargo publish -p import-lint` step above as the first full verification of that
crate.

## Future releases

For every release after v0.1.0:

1. Bump `[workspace.package] version` in the root `Cargo.toml` (both published
   crates inherit it via `version.workspace = true`) — including the pinned
   `import-lint-core = { path = "../core", version = "..." }` requirement in
   `crates/cli/Cargo.toml`, which needs bumping to match if it's pinned to an
   exact version rather than a compatible range. This is also what keeps the
   npm version equal to the crate version (docs/PLAN.md P4): `assemble.mjs`
   stamps the npm packages with the pushed tag's version, while the CLI binary
   reports its compiled-in `CARGO_PKG_VERSION` (from this same bump) at
   `--version` — the `npm-smoke` CI job asserts the two match, so an unbumped
   `Cargo.toml` fails the release rather than shipping a skewed npm package.
2. Update `README.md`'s Roadmap/changelog if applicable.
3. Follow the runbook above, substituting the new version for `v0.1.0` in the git
   tag.

`import-lint-core`'s API has no stability guarantee yet (pre-1.0) — treat any
breaking change to its public items as at least a minor version bump.

## npm distribution (one-time setup)

Before the first npm release (docs/PLAN.md N2), a maintainer needs to do two
things once — not per release:

1. **Create the npm org `import-lint`** (https://www.npmjs.com/org/create). It's
   needed for the six scoped platform packages, `@import-lint/<platform>`
   (docs/PLAN.md P1). Verify the org name is still available at creation time;
   if it's been taken, fall back to the `@importlint` scope (docs/PLAN.md §6) —
   only the shim's resolve strings and the package names change.
2. **Create an npm automation access token** (npmjs.com → Access Tokens →
   Generate New Token → Automation — automation tokens bypass 2FA prompts,
   which a CI job can't answer) and add it as the repo secret `NPM_TOKEN`
   (GitHub repo → Settings → Secrets and variables → Actions). `release.yml`'s
   `npm-publish` job reads it as `NODE_AUTH_TOKEN` for `npm publish`.

With both in place, pushing a `v*` tag ships npm automatically alongside
crates.io and the GitHub Release — no separate npm runbook step.
