# Releasing

ImportLint ships two crates to crates.io — `import-lint-core` (library) and
`import-lint` (the CLI binary, which depends on `import-lint-core` by version, not
just by path) — plus prebuilt binaries attached to a GitHub Release, built by
`.github/workflows/release.yml` on push of a `v*` tag. `crates/gen-fixture` is a
dev-only fixture generator and is never published (`publish = false`).

The same tag push also assembles, smoke-tests (on ubuntu/macos/windows), and
publishes the seven npm packages (`@import-lint/cli` plus the six
`@import-lint/*` platform packages, docs/PLAN-npm.md §2) with provenance — see
"npm distribution (one-time setup)" below for the setup this requires before
the first npm release.

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
   npm version equal to the crate version (docs/PLAN-npm.md P4): `assemble.mjs`
   stamps the npm packages with the pushed tag's version, while the CLI binary
   reports its compiled-in `CARGO_PKG_VERSION` (from this same bump) at
   `--version` — the `npm-smoke` CI job asserts the two match, so an unbumped
   `Cargo.toml` fails the release rather than shipping a skewed npm package.
2. Update `README.md`'s Roadmap/changelog if applicable.
3. If the config file shape changed, check `README.md`'s `## Config file`
   example against `crates/cli/src/init.rs`'s `gradual` template — the two are
   meant to read as the same text (docs/PLAN-init.md R-I4); the `init` round-trip
   unit tests catch schema drift but not prose drift between them. This
   prose-sync now extends to `docs/guides/`: `tutorial.md`'s hand-written
   config (a trimmed `standard`-preset equivalent) and `adoption.md`'s
   preset-comparison and playbook config snippets should still read as the
   same options/defaults as their corresponding `crates/cli/src/init.rs`
   template (docs/PLAN.md §3.6) — a schema change that isn't reflected in the
   guides is a doc bug even though nothing in CI catches it.
4. If this is the first release that ships `init` presets (i.e. `import-lint
   init --preset <name>` becomes usable via `npx @import-lint/cli@latest`,
   not just the local workspace build): update `docs/guides/tutorial.md`'s
   Setup section to scaffold with `npx @import-lint/cli init --preset
   standard` instead of hand-writing the config file, and drop
   `docs/guides/adoption.md`'s "copy the config blocks below by hand"
   fallback note — both currently exist only because `init` presets (M9)
   postdate the v0.1.2 build the guides were written and verified against.
5. Follow the runbook above, substituting the new version for `v0.1.0` in the git
   tag.

`import-lint-core`'s API has no stability guarantee yet (pre-1.0) — treat any
breaking change to its public items as at least a minor version bump.

## npm distribution (one-time setup)

CI publishes to npm via **OIDC trusted publishing** — no `NPM_TOKEN` secret, no
long-lived credentials. Each of the seven packages is configured on npmjs.com to
trust this repo's `release.yml` workflow; the `npm-publish` job exchanges a
GitHub-issued OIDC token for a short-lived publish credential at run time
(provenance attestations are generated automatically, and the job passes
`--provenance` explicitly as a belt-and-suspenders).

Setup happens once, in three steps. Steps 2–3 exist because **npm cannot
configure a trusted publisher for a package that has never been published**
(unlike PyPI's "pending publishers"; see npm/cli#8544) — so the very first npm
release is bootstrapped manually.

1. **Create the npm org `import-lint`** (https://www.npmjs.com/org/create). It's
   needed for the six scoped platform packages, `@import-lint/<platform>`
   (docs/PLAN-npm.md P1). Verify the org name is still available at creation time;
   if it's been taken, fall back to the `@importlint` scope (docs/PLAN-npm.md §6) —
   only the shim's resolve strings and the package names change.
   *(Done 2026-07-12.)*

2. **Bootstrap-publish the first npm release manually.** Push the release tag as
   normal. The `npm-publish` job will fail on this first run — expected, since
   no trusted publisher can exist yet. The `npm-assemble` and `npm-smoke` jobs
   still gate the artifacts, so publish their output by hand:

   ```sh
   # From the tag's workflow run, download the `npm-packages` artifact, then:
   unzip npm-packages.zip && tar -xf npm-packages.tar   # tar preserves the
                                                        # binaries' exec bits
   npm login   # interactive, 2FA

   # Platform packages first, main package last (docs/PLAN-npm.md P5) — the main
   # package must never be live while a platform package it pins is missing.
   # The leading "./" on each path is load-bearing: without it, `npm
   # publish` parses a single-slash argument like "npm/import-lint" as a
   # GitHub `owner/repo` shorthand instead of a local directory.
   for dir in ./npm/platform/darwin-arm64 ./npm/platform/darwin-x64 \
              ./npm/platform/linux-arm64-gnu ./npm/platform/linux-x64-gnu \
              ./npm/platform/linux-x64-musl ./npm/platform/win32-x64 \
              ./npm/import-lint; do
     npm publish "$dir" --access public
   done
   ```

   (No `--provenance` here — provenance requires publishing from CI, so the
   bootstrap versions won't carry attestations. Every later release will.)

3. **Configure a trusted publisher for each of the seven packages** (they exist
   on the registry now). Per package, on npmjs.com → package → Settings →
   Trusted Publisher → GitHub Actions:

   - Organization or user: `uhyo`
   - Repository: `import-lint`
   - Workflow filename: `release.yml` (basename only, with extension —
     not the `.github/workflows/` path)
   - Environment: leave empty
   - Allowed action: `npm publish`

   Or script it with npm ≥ 11.15.0 (enable the 5-minute "skip 2FA" window on
   npmjs.com first, and pause briefly between calls to avoid rate limiting):

   ```sh
   for pkg in @import-lint/darwin-arm64 @import-lint/darwin-x64 \
              @import-lint/linux-arm64-gnu @import-lint/linux-x64-gnu \
              @import-lint/linux-x64-musl @import-lint/win32-x64 \
              @import-lint/cli; do
     npm trust github "$pkg" --file release.yml --repo uhyo/import-lint \
       --allow-publish -y
     sleep 2
   done
   ```

   Optionally re-run the failed `npm-publish` job afterwards — it turns green by
   skipping every already-published package (the idempotency check), confirming
   the run wiring. The OIDC exchange itself is first exercised for real on the
   *next* release; if it fails there (the classic symptom is a misleading 404 —
   almost always a config-field mismatch or an npm CLI older than 11.5.1), fix
   the trusted-publisher config and re-run the job — publishes are idempotent.

After step 3, pushing a `v*` tag ships npm automatically alongside crates.io and
the GitHub Release — no tokens, no separate npm runbook step.

## VS Code extension release

The VS Code extension (`editors/vscode/`) is released **independently of the
linter** (docs/PLAN-lsp.md E7): its own tags (`vscode-v*`), its own workflow
(`.github/workflows/vscode-release.yml`), and its own version line. This is
deliberate — the extension is a thin client that runs whatever `import-lint`
binary the workspace has, so lockstep versioning with the linter would force
empty releases every time only the server changed. To keep the two pipelines
from cross-firing, the linter's `release.yml` trigger was narrowed from `v*`
to `v[0-9]*`, which `vscode-v*` tags don't match.

### One-time setup

1. **VS Code Marketplace.** Create publisher `uhyo` at
   https://marketplace.visualstudio.com/manage — it must match the
   `publisher` field in `editors/vscode/package.json`. Then create an Azure
   DevOps Personal Access Token scoped to that publisher, with the
   Marketplace → **Manage** scope (not a full-access/all-organizations
   token). Store it as the repo secret `VSCE_PAT`.

   Note: global Azure DevOps PATs are being retired in December 2026 (plan
   risk R4). A publisher-scoped Marketplace PAT is the current recommended
   approach and should keep working past that date, but expect Microsoft to
   move publishing auth to an Entra-ID-based flow at some point — treat this
   as a known follow-up, not a surprise, when it happens.

2. **Open VSX.** Create an Eclipse account and sign the [Open VSX publisher
   agreement](https://open-vsx.org/). Create the `uhyo` namespace:

   ```sh
   npx ovsx create-namespace uhyo -p <token>
   ```

   Generate an access token at https://open-vsx.org (Settings → Access
   Tokens) and store it as the repo secret `OVSX_PAT`.

### Per release

1. Bump `version` in `editors/vscode/package.json`, then run `npm install`
   (from `editors/vscode/`) to refresh `package-lock.json`.
2. Update `editors/vscode/CHANGELOG.md`.
3. Commit, tag `vscode-vX.Y.Z`, and push the tag:

   ```sh
   git tag vscode-v0.2.0 && git push origin vscode-v0.2.0
   ```

CI (`vscode-release.yml`) gates the tag's version against
`editors/vscode/package.json`, runs typecheck/test, packages the `.vsix`, and
publishes it to both the VS Code Marketplace and Open VSX (each with
`--skip-duplicate`, so a re-run of the job after a partial failure is safe),
then attaches the `.vsix` to a GitHub release for the tag.

Extension releases never touch crates.io, npm, or the linter's GitHub
Releases — and, per the tag-pattern note above, the linter's `v*` → `v[0-9]*`
narrowing means a `vscode-v*` push can never trigger `release.yml` by
accident.
