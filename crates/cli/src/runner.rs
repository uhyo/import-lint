//! Discovery + link orchestration (PLAN-v1.md §2.1 steps 2–4, §8, M2): walk, then
//! parse+extract and resolve in parallel (rayon + `AllocatorPool`), producing a
//! [`ModuleGraph`]. The check phase (PLAN-v1.md §2.1 step 5) is M3's job.
//!
//! M6 (watch mode, `docs/PLAN-v1.md` §7) needs these stages callable individually with a
//! caller-supplied [`ExtractionCache`], so a content-only file edit can skip
//! re-parsing every untouched file while still rebuilding the graph from scratch each
//! cycle (`crates/cli/src/watch.rs`'s `WatchSession`). [`run`] is the simple one-shot
//! entry point everything else (M2–M5 callers, tests) already uses; it is unchanged
//! and just wraps [`run_with_cache`] with a fresh, empty cache.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use import_lint::{FileModuleInfo, ModuleGraph, ProjectResolver, Provenance, SelfReferenceMode};
use oxc_allocator::AllocatorPool;
use oxc_parser::Parser as OxcParser;
use oxc_str::CompactStr;
use rayon::prelude::*;

use crate::overlay::Overlays;
use crate::source_type::source_type_for_path;
use crate::timing;

/// Options for one pipeline run.
#[derive(Debug, Clone, Default)]
pub struct RunnerOptions {
    /// Roots to walk for lint targets (PLAN-v1.md §2.1 step 2).
    pub roots: Vec<PathBuf>,
    /// Project root passed to the resolver (tsconfig discovery base, `cwd` option).
    /// Default: the current working directory.
    pub project_root: PathBuf,
    /// Path to the project's `tsconfig.json`, if any. Default:
    /// `<project_root>/tsconfig.json` if it exists, else `None` — see
    /// [`RunnerOptions::default_tsconfig`].
    pub tsconfig: Option<PathBuf>,
    /// How bare specifiers matching the importer's own package name are classified
    /// (spec §4.6, D5). Default: `SelfReferenceMode::External`.
    pub self_reference_mode: SelfReferenceMode,
    /// Extra glob patterns (config `exclude`, M5) applied on top of `.gitignore`
    /// during discovery, rooted at `project_root`. Default: none.
    pub exclude: Vec<String>,
}

impl RunnerOptions {
    /// This struct's documented default for `tsconfig`: `<project_root>/tsconfig.json`
    /// if it exists as a file, else `None`.
    pub fn default_tsconfig(project_root: &Path) -> Option<PathBuf> {
        let candidate = project_root.join("tsconfig.json");
        candidate.is_file().then_some(candidate)
    }
}

/// The signal a cache entry is validated against (L1, `docs/PLAN-lsp.md` §3): a disk
/// file's `(mtime, size)`, or — when an overlay covers the path — its overlay
/// version. Overlay content always wins over disk state when both could apply (an
/// overlaid file's `stat()` is never consulted at all), so the two variants are
/// mutually exclusive per path, never compared against each other: a file gaining or
/// losing its overlay naturally changes which variant [`current_stamp`] returns, which
/// already differs from any previously cached stamp and forces a re-extract.
#[derive(PartialEq)]
enum SourceStamp {
    Disk { mtime: SystemTime, size: u64 },
    Overlay { version: u64 },
}

/// One cached extraction result, keyed by file path (see [`ExtractionCache`]):
/// the [`SourceStamp`] at the time of extraction, plus the owned result. A cache entry
/// is reused only if a fresh [`current_stamp`] of the file still equals the cached
/// stamp — good enough to detect any real edit (disk or overlay), and cheap (no
/// content hashing).
pub(crate) struct CachedExtraction {
    stamp: SourceStamp,
    info: Arc<FileModuleInfo>,
}

/// Extraction cache keyed by absolute file path (M6, `docs/PLAN-v1.md` §7, watch-mode
/// brief D-W1): reused across pipeline runs so watch mode's content-only edit cycles
/// only re-parse the files that actually changed. `WatchSession` owns one and
/// invalidates entries for files it's told changed (belt-and-braces on top of the
/// `mtime`/`size` check here, since some filesystems have coarse `mtime` resolution).
pub(crate) type ExtractionCache = HashMap<PathBuf, CachedExtraction>;

/// A batch of extraction results plus how many of them were actual cache misses
/// (files that had to be re-parsed) — watch mode reports and tests this count
/// directly (M6 brief D-W3). `pub(crate)` (not `pub`): only `watch.rs`'s
/// content-only edit cycle needs to call [`extract_with_cache`] directly, to seed
/// [`extract_and_link_from`] without going through [`run_with_cache`]'s "structural"
/// (re-walk + fresh resolver) path.
pub(crate) struct Extracted {
    pub(crate) files: Vec<Arc<FileModuleInfo>>,
    /// Number of `paths` that were NOT served from `cache` this call (i.e. were
    /// missing, stale, or unreadable-for-stat and thus attempted for extraction).
    pub(crate) extracted_files: usize,
}

/// Outcome of a full extract+link pass: the resulting graph plus the total number of
/// files actually (re-)extracted (cache misses) across the whole pass, including any
/// fixpoint-discovered files outside the walked set.
pub struct RunOutcome {
    pub graph: ModuleGraph,
    pub extracted_files: usize,
}

/// Run the discovery + link pipeline: walk `options.roots`, parse+extract every file
/// found, resolve every specifier they reference, and extract+resolve any internal
/// resolution target outside the walked set too (so the one-hop check in M3 can look
/// up its export table and, transitively, the files reachable through its own
/// `star_exports`) — repeating until a fixpoint is reached. Never panics on a single
/// file's read/parse/resolve failure; such files are skipped with a stderr warning.
pub fn run(options: &RunnerOptions) -> ModuleGraph {
    let mut cache = ExtractionCache::new();
    run_with_cache(options, &mut cache, &Overlays::default()).graph
}

/// Same pipeline as [`run`], but extraction is served through `cache` (populated as
/// it goes) instead of a throwaway one-shot cache. This is the "structural" build
/// path (M6 brief D-W1): it walks `options.roots` from scratch and constructs a fresh
/// [`ProjectResolver`], so it's what watch mode calls on startup and whenever the
/// filesystem layout may have changed (file added/removed/renamed, `package.json`,
/// config, or tsconfig edited).
pub(crate) fn run_with_cache(
    options: &RunnerOptions,
    cache: &mut ExtractionCache,
    overlays: &Overlays,
) -> RunOutcome {
    let lint_target_paths = timing::phase("walk", || {
        crate::walk::walk_with_excludes(
            &options.roots,
            Some(options.project_root.as_path()),
            &options.exclude,
        )
    });

    let pool = AllocatorPool::new(rayon::current_num_threads());
    let initial = extract_with_cache(&lint_target_paths, &pool, cache, overlays);
    let resolver = build_resolver_from_files(options, &lint_target_paths, &initial.files);
    extract_and_link_from(
        &lint_target_paths,
        initial,
        &resolver,
        &pool,
        cache,
        overlays,
    )
}

/// Build a fresh [`ProjectResolver`] for `options`, seeding the ambient-module
/// registry (D6) from `initial_files` — the already-extracted walked set, restricted
/// to lint targets inside it (same rule as the original M2 `run`: a `.d.ts` only
/// reached later via the fixpoint loop never contributes ambient declarations).
///
/// Split out from [`run_with_cache`] so watch mode's "structural" reload path
/// (`crates/cli/src/watch.rs`) can build the resolver once and reuse the same
/// `initial_files` extraction as the seed for [`extract_and_link_from`], instead of
/// extracting the walked set twice.
pub(crate) fn build_resolver_from_files(
    options: &RunnerOptions,
    lint_target_paths: &[PathBuf],
    initial_files: &[Arc<FileModuleInfo>],
) -> ProjectResolver {
    let lint_targets: HashSet<PathBuf> = lint_target_paths.iter().cloned().collect();
    let files_by_path: HashMap<PathBuf, Arc<FileModuleInfo>> = initial_files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let ambient_modules = build_ambient_registry(&files_by_path, &lint_targets);

    ProjectResolver::new(
        &options.project_root,
        options.tsconfig.clone(),
        ambient_modules,
        options.self_reference_mode,
    )
}

/// Extract+resolve `lint_target_paths` against `resolver` to a fixpoint (PLAN-v1.md
/// §2.1 steps 3–4): any internal resolution target outside the walked set is
/// extracted and resolved too, so the one-hop check (M3) can look up its export
/// table. `initial` is the already-extracted walked set (from [`extract_with_cache`]
/// on `lint_target_paths`) — passing it in rather than re-deriving it from
/// `lint_target_paths` lets callers reuse whatever extraction they already did (e.g.
/// [`build_resolver_from_files`]'s ambient-registry pass) without a second cache
/// lookup/extraction round, and keeps `extracted_files` counts from being
/// double-counted.
///
/// This is the piece watch mode's content-only edit cycles call directly, reusing the
/// previous walk result and [`ProjectResolver`] across cycles (M6 brief D-W1): only
/// `cache` changes between cycles (entries for edited files invalidated by the
/// caller), so re-running this is cheap when nothing relevant changed.
pub(crate) fn extract_and_link_from(
    lint_target_paths: &[PathBuf],
    initial: Extracted,
    resolver: &ProjectResolver,
    pool: &AllocatorPool,
    cache: &mut ExtractionCache,
    overlays: &Overlays,
) -> RunOutcome {
    let lint_targets: HashSet<PathBuf> = lint_target_paths.iter().cloned().collect();

    // `attempted` guards the fixpoint loop against cycles and against retrying a
    // file that failed to read/parse on a previous pass: once a path has been
    // attempted (successfully or not) in this call, it's never re-extracted.
    let mut attempted: HashSet<PathBuf> = lint_targets.clone();
    let mut files: HashMap<PathBuf, Arc<FileModuleInfo>> = HashMap::new();
    let mut resolutions: HashMap<(PathBuf, CompactStr), Provenance> = HashMap::new();
    let mut extracted_files = initial.extracted_files;

    let mut pending = initial.files;
    timing::phase(&format!("files_index({} files)", pending.len()), || {
        for file in &pending {
            files.insert(file.path.clone(), file.clone());
        }
    });

    loop {
        let round = timing::phase(&format!("resolve({} files)", pending.len()), || {
            resolve_pairs(&pending, resolver)
        });
        timing::phase(&format!("resolutions_merge({} pairs)", round.len()), || {
            resolutions.extend(round);
        });

        let mut new_targets: Vec<PathBuf> = resolutions
            .values()
            .filter_map(|provenance| match provenance {
                Provenance::Internal(path) if !attempted.contains(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        new_targets.sort();
        new_targets.dedup();

        if new_targets.is_empty() {
            break;
        }
        attempted.extend(new_targets.iter().cloned());

        let next = extract_with_cache(&new_targets, pool, cache, overlays);
        extracted_files += next.extracted_files;
        pending = next.files;
        for file in &pending {
            files.insert(file.path.clone(), file.clone());
        }
    }

    let graph = timing::phase("graph_build", || {
        ModuleGraph::build(files.into_values().collect(), resolutions, lint_targets)
    });
    RunOutcome {
        graph,
        extracted_files,
    }
}

/// Extract every path in `paths`, serving already-extracted, unchanged files from
/// `cache` and only actually parsing (rayon + `AllocatorPool`, PLAN-v1.md §8) the ones
/// that are missing or whose `(mtime, size)` no longer matches the cached entry.
/// Freshly extracted files are inserted into `cache` before returning.
///
/// `pub(crate)`: exposed so `watch.rs`'s content-only edit cycle can extract the
/// (unchanged) walked set through the cache directly, as the seed for
/// [`extract_and_link_from`], without re-walking or rebuilding the resolver.
pub(crate) fn extract_with_cache(
    paths: &[PathBuf],
    pool: &AllocatorPool,
    cache: &mut ExtractionCache,
    overlays: &Overlays,
) -> Extracted {
    let mut files: Vec<Arc<FileModuleInfo>> = Vec::with_capacity(paths.len());
    let mut miss_paths: Vec<PathBuf> = Vec::new();

    timing::phase(&format!("stat({} paths)", paths.len()), || {
        for path in paths {
            match current_stamp(path, overlays) {
                Some(stamp) => {
                    let cache_hit = cache.get(path).is_some_and(|cached| cached.stamp == stamp);
                    if cache_hit {
                        // `cache_hit` guarantees `cache.get(path)` is `Some`.
                        files.push(cache.get(path).unwrap().info.clone());
                        continue;
                    }
                    miss_paths.push(path.clone());
                }
                None => {
                    // Can't stat and no overlay covers it: vanished between walking
                    // and extracting, a permission error, or (in principle) a platform
                    // without `mtime` support. Always treat as a miss —
                    // `extract_one`'s own read-error path reports/skips it — and drop
                    // any stale entry so a later re-appearance of the file at this
                    // path is picked up fresh rather than silently reusing old
                    // content.
                    cache.remove(path);
                    miss_paths.push(path.clone());
                }
            }
        }
    });

    let extracted_files = miss_paths.len();
    let newly_extracted = timing::phase(&format!("parse({extracted_files} files)"), || {
        extract_files(&miss_paths, pool, overlays)
    });
    for info in newly_extracted {
        let arc = Arc::new(info);
        if let Some(stamp) = current_stamp(&arc.path, overlays) {
            cache.insert(
                arc.path.clone(),
                CachedExtraction {
                    stamp,
                    info: arc.clone(),
                },
            );
        }
        files.push(arc);
    }

    Extracted {
        files,
        extracted_files,
    }
}

/// [`SourceStamp`] for `path` right now (L1, `docs/PLAN-lsp.md` §3): the overlay version
/// if `overlays` covers `path`, else the disk `(mtime, size)` — overlay content always
/// wins, so an overlaid file's `stat()` is never even consulted.
fn current_stamp(path: &Path, overlays: &Overlays) -> Option<SourceStamp> {
    if let Some(version) = overlays.version(path) {
        return Some(SourceStamp::Overlay { version });
    }
    stat(path).map(|(mtime, size)| SourceStamp::Disk { mtime, size })
}

fn stat(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

/// Parse+extract `paths` in parallel (rayon + `AllocatorPool` arena reuse, PLAN-v1.md
/// §8): one arena per worker, reset after each file, only the owned
/// `FileModuleInfo` crosses the parallel boundary. A file that can't be read or
/// fails to parse is skipped with a stderr warning rather than aborting the run.
fn extract_files(
    paths: &[PathBuf],
    pool: &AllocatorPool,
    overlays: &Overlays,
) -> Vec<FileModuleInfo> {
    paths
        .par_iter()
        .filter_map(|path| extract_one(path, pool, overlays))
        .collect()
}

fn extract_one(path: &Path, pool: &AllocatorPool, overlays: &Overlays) -> Option<FileModuleInfo> {
    // Every walked path already has a recognized extension (`walk()` filters for
    // it); this branch only fires for a fixpoint-discovered internal resolution
    // target with an extension ImportLint doesn't parse (e.g. a `.json` data
    // import) — skip it the same way an unparseable file is skipped.
    let Some(source_type) = source_type_for_path(path) else {
        eprintln!(
            "import-lint: {}: unrecognized file extension, skipping",
            path.display()
        );
        return None;
    };

    // An overlay (an open editor buffer's in-memory content, L1) always wins over
    // disk content when both exist for `path`.
    let source_text: Arc<str> = match overlays.content(path) {
        Some(content) => content,
        None => match fs::read_to_string(path) {
            Ok(text) => Arc::from(text),
            Err(err) => {
                eprintln!(
                    "import-lint: cannot read {}: {err}, skipping",
                    path.display()
                );
                return None;
            }
        },
    };

    let allocator = pool.get();
    // Pre-flight parse to detect syntax errors before handing the source to
    // `extract_file` (which has no diagnostics in its return type by design — see
    // `main.rs`'s `inspect` command for the same accepted double-parse trade-off).
    let preflight = OxcParser::new(&allocator, &source_text, source_type).parse();
    if preflight.panicked || preflight.diagnostics.has_errors() {
        eprintln!("import-lint: failed to parse {}, skipping:", path.display());
        for diagnostic in preflight.diagnostics.errors() {
            eprintln!("  {diagnostic}");
        }
        return None;
    }

    Some(import_lint::extract_file(
        path,
        &source_text,
        source_type,
        &allocator,
    ))
}

/// Resolve every distinct `(file, specifier)` pair reachable from `files` in
/// parallel. `ProjectResolver::resolve` is thread-safe by design (PLAN-v1.md §8): one
/// shared resolver, never a per-file one.
fn resolve_pairs(
    files: &[Arc<FileModuleInfo>],
    resolver: &ProjectResolver,
) -> HashMap<(PathBuf, CompactStr), Provenance> {
    let pairs: Vec<(&Path, &CompactStr)> = files
        .iter()
        .flat_map(|file| {
            file.specifiers
                .iter()
                .map(move |specifier| (file.path.as_path(), specifier))
        })
        .collect();

    pairs
        .par_iter()
        .map(|(path, specifier)| {
            let provenance = resolver.resolve(path, specifier);
            ((path.to_path_buf(), (*specifier).clone()), provenance)
        })
        .collect()
}

/// Build the ambient-module registry (D6): specifier -> declaring `.d.ts` file,
/// restricted to the walked (lint target) set. Deterministic on conflict — iterate
/// candidate files sorted by path, first declaration wins.
fn build_ambient_registry(
    files: &HashMap<PathBuf, Arc<FileModuleInfo>>,
    lint_targets: &HashSet<PathBuf>,
) -> HashMap<CompactStr, PathBuf> {
    let mut declaration_files: Vec<&Arc<FileModuleInfo>> = files
        .values()
        .filter(|file| lint_targets.contains(&file.path))
        .filter(|file| {
            source_type_for_path(&file.path).is_some_and(|t| t.is_typescript_definition())
        })
        .collect();
    declaration_files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut registry = HashMap::new();
    for file in declaration_files {
        for specifier in &file.ambient_modules {
            registry
                .entry(specifier.clone())
                .or_insert_with(|| file.path.clone());
        }
    }
    registry
}
