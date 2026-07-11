# Benchmarks (M7)

Performance targets, per `docs/PLAN.md` §8:

- Cold lint of 5,000 files in **< 2 s**, 10,000 files in **< 4 s**.
- Watch-mode incremental cycle **< 100 ms** for a single-file edit in a 10k-file
  project.

All numbers below are measured on the machine described in
[Machine](#machine), dated 2026-07-11. Re-run with the exact commands listed to
reproduce.

## Machine

- CPU: AMD Ryzen 7 PRO 6850U with Radeon Graphics (16 logical cores, `nproc` = 16)
- OS: `Linux 6.18.33.2-microsoft-standard-WSL2` — **WSL2** (Ubuntu userland on
  Windows), not bare-metal Linux. WSL2's virtualized I/O path and 9p/DrvFS
  overhead for cross-filesystem access don't apply here (the fixtures live on
  the WSL2 ext4 filesystem, not `/mnt/c`), but scheduling/virtualization
  overhead generally still differs from bare metal — treat these as directional,
  not absolute, numbers.
- `rustc 1.95.0`, `cargo 1.95.0`, release profile (`cargo build --release`)
- `hyperfine 1.20.0` (`cargo install hyperfine --locked` — not preinstalled in
  this environment; `scripts/bench.sh` falls back to a 5-run `time` loop if
  `hyperfine` isn't on `PATH`)

## Cold lint: 5,000 / 10,000 files (target: < 2 s / < 4 s)

Command:

```sh
scripts/bench.sh
```

This builds `--release`, generates (or reuses, keyed by file count + seed)
synthetic project trees via `gen-fixture` into `/tmp/import-lint-bench-cache`,
and times `import-lint <tree>` with `hyperfine --warmup 1 --ignore-failure`
(the target trees intentionally contain real `@package`/`@private`
violations — see `crates/gen-fixture/src/lib.rs` — so `import-lint` always
exits 1; hyperfine is told to ignore that).

| Tree | Actual `.ts` files | Mean ± σ | Range (min … max) | Runs |
|---|---|---|---|---|
| 5k (`--files 5000`) | 5,158 | **157.5 ms ± 15.9 ms** | 141.7 ms … 199.6 ms | 17 |
| 10k (`--files 10000`) | 10,261 | **322.7 ms ± 20.7 ms** | 300.1 ms … 354.5 ms | 10 |

**Both targets are met with a large margin**: ~13x headroom at 5k (157.5 ms vs.
2,000 ms), ~12x headroom at 10k (322.7 ms vs. 4,000 ms). File counts are
slightly above the requested 5,000/10,000 because `gen-fixture` adds
`index.ts` barrel files (one per directory, ~150-260 of them) and 2 ambient
`.d.ts` files on top of the requested content-file count — see
`crates/gen-fixture/src/lib.rs`'s module doc comment for the fixture shape.

Repeated runs (`scripts/bench.sh` invoked twice back to back) were consistent
within noise: a first pass without `--compare-eslint` measured 185.9 ms
(5k) / 336.8 ms (10k); a second pass measured 157.5 ms / 322.7 ms. Both
comfortably clear the targets.

## ESLint comparison (`--compare-eslint`, 5k tree only)

Command:

```sh
scripts/bench.sh --compare-eslint
```

This additionally times the reference `eslint-plugin-import-access` (checked
out read-only at `~/repos/eslint-plugin-import-access`) over the same 5k tree,
in a throwaway npm project created *outside* that checkout
(`mktemp -d -t import-lint-bench-eslint-XXXXXX`): a minimal `package.json`, an
`npm install --save-exact` of the exact `eslint`/`@typescript-eslint/parser`/
`typescript` versions already installed in the reference checkout's own
`node_modules` (so the plugin's own dependency resolution — `@typescript-eslint/utils`,
`minimatch`, `tsutils` — is satisfied from *that* checkout's `node_modules`
via Node's symlink-realpath resolution) plus
`eslint-plugin-import-access@file:$REFERENCE_CHECKOUT`, and a generated
`eslint.config.mjs` using `@typescript-eslint/parser` with
`parserOptions.projectService: true` and `tsconfigRootDir` pointed at the
fixture tree.

**Bug found and fixed while wiring this up**: ESLint 9's flat config rejects
lint targets outside the config file's "base path" — pointing `--config` at
the throwaway project's `eslint.config.mjs` while passing the *absolute path*
of the fixture tree (which lives in a different directory) makes ESLint fail
instantly with "all files matching the glob pattern are ignored" (exit 2,
~0.5 s, zero files actually linted). The first (wrong) measurement looked like
a suspiciously fast ~500 ms "ESLint" run — that was this failure, not a real
lint pass. The fix (now in `scripts/bench.sh`): run with `cwd` set to the
fixture directory and `.` as the lint target, while still passing an absolute
`--config` path. Verified this actually lints everything (3,497 lines of
output, 1,242 real diagnostics, ~25 s) before trusting the timed numbers below.

| Tree | Files | Tool | Mean ± σ | Range | Runs |
|---|---|---|---|---|---|
| 5k | 5,158 | `import-lint` (this run) | 157.5 ms ± 15.9 ms | 141.7 ms … 199.6 ms | 17 |
| 5k | 5,158 | `eslint-plugin-import-access` (reference, type-aware) | **24.365 s ± 0.422 s** | 24.072 s … 24.849 s | 3 |

**import-lint is ~155x faster** than the reference ESLint plugin on the same
5,000-file tree (157.5 ms vs. 24.365 s). This isn't a fully apples-to-apples
comparison — the reference plugin runs through full TypeScript-type-aware
ESLint parsing (`projectService: true`), which does meaningfully more work
per file (a real `tsc` program, not just a syntax parse) — but it's the
realistic "migrate from the ESLint plugin" comparison a user would actually
experience, which is the point of the README's headline number.

If `npm`/network access isn't available, `scripts/bench.sh --compare-eslint`
degrades gracefully: it prints manual instructions (build the reference repo,
set up a throwaway npm project, run `hyperfine` by hand) instead of failing
the rest of the script.

## Criterion micro-benchmark: `extract()`

Command:

```sh
cargo bench -p import_lint --bench extract
```

Benchmarks `import_lint::extract_file` over a hand-written ~150-line
representative `.ts` file (`crates/core/benches/extract.rs`) mixing imports,
`const`/`function`/`class`/`interface`/`type`/`namespace`/`enum` exports, a
default export, re-exports (`export { x as y } from`, `export * from`,
`export * as ns from`), and JSDoc `@package`/`@private` tags on roughly a
third of the exported declarations.

```
extract/representative_file
                        time:   [44.647 µs 46.809 µs 49.569 µs]
```

**~47 µs per representative file.** At that rate, extraction alone accounts
for well under 1 second even for 10,000 files if it were fully serial
(10,000 × 47 µs ≈ 0.47 s) — in practice it's rayon-parallelized across cores
(PLAN.md §8), so the cold-lint numbers above spend most of their wall time
elsewhere (discovery, resolution, graph assembly, report rendering), not in
`extract()` itself.

## Watch-mode single-edit cycle at 10k files (target: < 100 ms)

**Target is currently missed** — measured cycle time is consistently in the
**155–220 ms** range, roughly 1.6–2x over the 100 ms target. This is a real,
reproducible finding, documented here for a follow-up rather than fixed now
(see [Why, and what a fix would look like](#why-and-what-a-fix-would-look-like)).

Command (must be release mode — the debug-build pipeline is far slower than
100 ms even for a no-op cycle):

```sh
cargo test --release -p import_lint_cli --test watch -- --ignored watch_cycle_timing_10k --nocapture
```

Three consecutive runs:

| Run | Cycle duration |
|---|---|
| 1 | 167.3 ms |
| 2 | 221.5 ms |
| 3 | 170.5 ms |

The test (`crates/cli/tests/watch.rs::watch_cycle_timing_10k`, `#[ignore]`d so
it doesn't run in `cargo test --workspace` or CI) generates a 10,261-file tree
via `gen_fixture::generate` (the library function, not the binary — added as a
dev-dependency of `import_lint_cli`), builds a `WatchSession` (whose
constructor performs the untimed initial full run), edits one content file,
and times a single `WatchSession::run_cycle([ContentEdit(...)])` call. It
`assert!`s `< 100 ms` per the M7 brief, so it currently fails when run — that
failure is the expected, documented state of this finding, not a bug in the
test.

### Where the time goes

`runner.rs` and `report.rs` now have permanent, zero-cost-when-disabled
per-phase timing instrumentation (`crates/cli/src/timing.rs`): set
`IMPORT_LINT_TIMING=1` (any non-empty value) to print `[timing] <phase>: <ms>`
to stderr for each instrumented phase.

```sh
IMPORT_LINT_TIMING=1 cargo test --release -p import_lint_cli --test watch -- --ignored watch_cycle_timing_10k --nocapture
```

One representative cycle (total 169.8 ms, matching the un-instrumented range
above — note the `eprintln!` calls this env var turns on have their own small
but non-negligible overhead, so treat the *proportions* below as reliable and
the *absolute total* of instrumented-run numbers as slightly inflated versus
the clean numbers in the table above):

| Phase | Time | What it is |
|---|---|---|
| `stat(10261 paths)` | 24.5 ms | `mtime`/`size` check against the extraction cache for every walked path (only 1 is actually re-parsed) |
| `parse(1 files)` | 0.1 ms | the one file that actually changed |
| `files_index(10261 files)` | 3.0 ms | rebuilding the `PathBuf -> Arc<FileModuleInfo>` map from scratch |
| `resolve(10261 files)` | 16.3 ms | re-resolving every specifier of every file (the shared resolver's cache makes each individual resolution cheap, but it's still O(files × specifiers) call overhead) |
| `resolutions_merge(40135 pairs)` | 9.0 ms | merging the resolved pairs into the cycle's `resolutions` map |
| `graph_build` | 44.7 ms | `ModuleGraph::build`: rebuilding `files`, `importers`, `star_importers` reverse indexes from scratch |
| `rechecked_files_count` | 1.6 ms | counting lint targets present in the rebuilt graph |
| `linted_files_clone` | 0.2 ms | cloning the lint-target path list for the report |
| `check_graph` (nested in `build_report_total`) | 35.2 ms | the rule engine over every lint target |
| `build_report_total` | 48.7 ms | `check_graph` above plus diagnostic line/col lookup, sort, `--quiet` filtering |

Summed instrumented phases (excluding the double-counted nested `check_graph`)
account for ~148 ms of the ~170 ms total; the remaining ~20 ms is consistent
with allocator churn from dropping the *previous* cycle's `ModuleGraph` (several
10k+/40k+-entry `HashMap`s) at the end of `WatchSession::recheck`, which isn't
separately instrumented.

### Why, and what a fix would look like

This is **by design**, not a bug: `crates/cli/src/watch.rs`'s module doc and
`WatchSession::run_cycle` doc comment are explicit that watch mode always does
a full re-walk-free-but-otherwise-full extract(-stale-only)+link+check pass
every cycle (M6 brief D-W1) — extraction is cached per-file, but resolution,
graph assembly, and the check phase always run over the *entire* project, not
just files reachable from the edit. That's a deliberate simplicity trade-off
from M6, and it's what's costing ~150+ ms here: `graph_build` (44.7 ms) and
`check_graph`/`build_report_total` (48.7 ms) alone are the two biggest phases,
both O(all files), even though only one file changed.

A real fix is a genuine incremental-invalidation redesign — e.g. keeping the
previous cycle's `ModuleGraph` and `resolutions` map alive, computing a dirty
set from the reverse-edge index (`ModuleGraph::importers`/`star_importers`,
which already exist for exactly this purpose per their doc comments) and only
re-resolving/re-checking files reachable from it — which is out of scope for
an M7 benchmarking pass per the "only implement optimizations if the fix is
small and obvious" guidance. A smaller, lower-risk interim option worth
evaluating in a follow-up: swap the hot per-cycle-rebuilt `HashMap`s (`ModuleGraph::files`/
`resolutions`/`importers`/`star_importers`) to a faster (non-DoS-resistant)
hasher, since `graph_build` and the merge/index phases above are dominated by
`HashMap` insert cost over 10k-40k entries with `PathBuf`/tuple keys under the
default SipHash — but that also touches public struct field types used across
many call sites and tests, so it needs its own follow-up rather than a
same-milestone patch.

The `< 100 ms` assertion in `watch_cycle_timing_10k` is left in place
(matching the PLAN.md §8 target) rather than loosened to the currently
measured number, so this test keeps failing loudly — and gets re-profiled with
the instrumentation above — until a follow-up closes the gap.

## Reproducing everything

```sh
cargo build --release
scripts/bench.sh                    # cold lint, 5k + 10k
scripts/bench.sh --compare-eslint   # + reference ESLint plugin, 5k only (slow, ~30s)
cargo bench -p import_lint --bench extract
cargo test --release -p import_lint_cli --test watch -- --ignored watch_cycle_timing_10k --nocapture
```
