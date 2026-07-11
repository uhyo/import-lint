# Releasing

ImportLint ships two crates to crates.io — `import-lint-core` (library) and
`import-lint` (the CLI binary, which depends on `import-lint-core` by version, not
just by path) — plus prebuilt binaries attached to a GitHub Release, built by
`.github/workflows/release.yml` on push of a `v*` tag. `crates/gen-fixture` is a
dev-only fixture generator and is never published (`publish = false`).

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
                                            # which builds binaries for all targets
                                            # and creates the GitHub Release
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
   exact version rather than a compatible range.
2. Update `README.md`'s Roadmap/changelog if applicable.
3. Follow the runbook above, substituting the new version for `v0.1.0` in the git
   tag.

`import-lint-core`'s API has no stability guarantee yet (pre-1.0) — treat any
breaking change to its public items as at least a minor version bump.
