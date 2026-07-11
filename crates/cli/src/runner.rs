//! Discovery + link orchestration (PLAN.md §2.1 steps 2–4, §8, M2): walk, then
//! parse+extract and resolve in parallel (rayon + `AllocatorPool`), producing a
//! [`ModuleGraph`]. The check phase (PLAN.md §2.1 step 5) is M3's job.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use import_lint::{FileModuleInfo, ModuleGraph, ProjectResolver, Provenance, SelfReferenceMode};
use oxc_allocator::AllocatorPool;
use oxc_parser::Parser as OxcParser;
use oxc_str::CompactStr;
use rayon::prelude::*;

use crate::source_type::source_type_for_path;

/// Options for one pipeline run.
#[derive(Debug, Clone, Default)]
pub struct RunnerOptions {
    /// Roots to walk for lint targets (PLAN.md §2.1 step 2).
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

/// Run the discovery + link pipeline: walk `options.roots`, parse+extract every file
/// found, resolve every specifier they reference, and extract+resolve any internal
/// resolution target outside the walked set too (so the one-hop check in M3 can look
/// up its export table and, transitively, the files reachable through its own
/// `star_exports`) — repeating until a fixpoint is reached. Never panics on a single
/// file's read/parse/resolve failure; such files are skipped with a stderr warning.
pub fn run(options: &RunnerOptions) -> ModuleGraph {
    let lint_target_paths = crate::walk::walk_with_excludes(
        &options.roots,
        Some(options.project_root.as_path()),
        &options.exclude,
    );
    let lint_targets: HashSet<PathBuf> = lint_target_paths.iter().cloned().collect();

    let pool = AllocatorPool::new(rayon::current_num_threads());

    // `attempted` guards the fixpoint loop against cycles and against retrying a
    // file that failed to read/parse on a previous pass: once a path has been
    // attempted (successfully or not), it's never re-extracted.
    let mut attempted: HashSet<PathBuf> = lint_target_paths.iter().cloned().collect();
    let mut files: HashMap<PathBuf, Arc<FileModuleInfo>> = HashMap::new();
    let mut resolutions: HashMap<(PathBuf, CompactStr), Provenance> = HashMap::new();

    let mut pending: Vec<Arc<FileModuleInfo>> = extract_files(&lint_target_paths, &pool)
        .into_iter()
        .map(Arc::new)
        .collect();
    for file in &pending {
        files.insert(file.path.clone(), file.clone());
    }

    // Ambient-module registry (D6), built from the walked set only: a `.d.ts` only
    // reached later via the fixpoint loop below never contributes ambient module
    // declarations to the resolver. This is a deliberate simplification — ambient
    // modules are project-authored and expected to live inside the walked tree.
    let ambient_modules = build_ambient_registry(&files, &lint_targets);

    let resolver = ProjectResolver::new(
        &options.project_root,
        options.tsconfig.clone(),
        ambient_modules,
        options.self_reference_mode,
    );

    loop {
        resolutions.extend(resolve_pairs(&pending, &resolver));

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

        pending = extract_files(&new_targets, &pool)
            .into_iter()
            .map(Arc::new)
            .collect();
        for file in &pending {
            files.insert(file.path.clone(), file.clone());
        }
    }

    ModuleGraph::build(files.into_values().collect(), resolutions, lint_targets)
}

/// Parse+extract `paths` in parallel (rayon + `AllocatorPool` arena reuse, PLAN.md
/// §8): one arena per worker, reset after each file, only the owned
/// `FileModuleInfo` crosses the parallel boundary. A file that can't be read or
/// fails to parse is skipped with a stderr warning rather than aborting the run.
fn extract_files(paths: &[PathBuf], pool: &AllocatorPool) -> Vec<FileModuleInfo> {
    paths
        .par_iter()
        .filter_map(|path| extract_one(path, pool))
        .collect()
}

fn extract_one(path: &Path, pool: &AllocatorPool) -> Option<FileModuleInfo> {
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

    let source_text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!(
                "import-lint: cannot read {}: {err}, skipping",
                path.display()
            );
            return None;
        }
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
/// parallel. `ProjectResolver::resolve` is thread-safe by design (PLAN.md §8): one
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
