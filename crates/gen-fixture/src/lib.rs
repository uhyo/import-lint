//! Deterministic synthetic TypeScript project generator (PLAN.md §8, M7): produces a
//! realistic-shaped tree for `import-lint`'s end-to-end benchmarks (`scripts/bench.sh`)
//! and the watch-mode timing test (`crates/cli/tests/watch.rs`).
//!
//! Shape (see [`GenOptions`] and [`generate`] for the exact knobs):
//! - `tsconfig.json` (`baseUrl` + two `paths` aliases) and `package.json` at the root.
//! - Content files distributed across a directory tree ~50 files/directory, 3-4
//!   levels deep, each with 2-5 exports (functions/consts/classes/interfaces/types
//!   plus an occasional default export), ~30% of which carry `/** @package */` or
//!   `/** @private */` JSDoc.
//! - Every directory (leaf and non-leaf) gets an `index.ts` barrel of `export * from`
//!   re-exports, chaining up to the root — so resolving/checking a barrel import walks
//!   a real multi-hop `export *` chain.
//! - Content files import from: siblings in the same directory, other directories by
//!   relative path (some of these are real `@package`/`@private` violations across
//!   unrelated directories), directory barrels, the tsconfig path aliases, and (a
//!   handful) two ambient `declare module` `.d.ts` files.
//!
//! No randomness crate: [`Lcg`] is a tiny deterministic linear congruential
//! generator, seeded by [`GenOptions::seed`], so two calls with the same
//! `(files, seed)` produce byte-identical output.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

/// Inputs to [`generate`].
#[derive(Debug, Clone, Copy)]
pub struct GenOptions {
    /// Target number of *content* files (files with exports/imports — barrels and
    /// the ambient `.d.ts` files are extra, on top of this count).
    pub files: usize,
    /// Seed for the deterministic LCG. Same `(files, seed)` always produces the
    /// same tree.
    pub seed: u64,
}

/// What [`generate`] produced, for the caller to report.
#[derive(Debug, Clone, Copy)]
pub struct GenResult {
    /// Content files (the `files` request, possibly rounded up slightly by the
    /// directory-distribution math).
    pub content_files: usize,
    /// `index.ts` barrel files (one per directory, leaf and non-leaf).
    pub barrel_files: usize,
    /// Ambient `.d.ts` files.
    pub ambient_files: usize,
}

impl GenResult {
    pub fn total_files(&self) -> usize {
        self.content_files + self.barrel_files + self.ambient_files
    }
}

/// A tiny deterministic PRNG (splitmix64-style LCG) — no `rand` dependency, since
/// this crate is intentionally dependency-free (see `Cargo.toml`).
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        // Avoid an all-zero state, which would produce an all-zero stream forever.
        Lcg(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next_u64(&mut self) -> u64 {
        // Numerical Recipes LCG constants, then a splitmix-style output mix so
        // low bits aren't low-quality (LCG low bits have short periods).
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`. `n == 0` returns `0` (callers guard for that separately
    /// where it matters).
    fn gen_range(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// Uniform in `[0.0, 1.0)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// One directory node in the synthetic tree. `components` is the path relative to
/// the output root, e.g. `["src", "g0", "g1"]`. Leaf nodes (`is_leaf`) hold content
/// files directly; every node (leaf or not) gets its own `index.ts` barrel.
struct DirNode {
    components: Vec<String>,
    children: Vec<usize>,
    files: Vec<usize>,
    is_leaf: bool,
}

/// One export site: its (globally unique) name and optional JSDoc access tag.
struct ExportMeta {
    name: String,
    tag: Option<&'static str>,
}

/// One content file's metadata, computed in a first pass so the second (writing)
/// pass can freely reference any other file's exports without forward-reference
/// ordering constraints.
struct FileMeta {
    dir: usize,
    /// Full path components including the filename (no extension), e.g.
    /// `["src", "g0", "g1", "file7"]`.
    components: Vec<String>,
    exports: Vec<ExportMeta>,
    default_export: Option<ExportMeta>,
}

const AMBIENT_MODULES: [(&str, &[&str]); 2] = [
    ("synthetic-external-lib", &["helper", "VERSION"]),
    ("synthetic-other-lib", &["otherHelper", "OTHER_VERSION"]),
];

/// Generate a synthetic project at `out_dir` (created if it doesn't exist; existing
/// contents are not cleaned first — callers generating into a fresh temp/cache dir
/// don't need that, and it keeps this function non-destructive).
pub fn generate(out_dir: &Path, opts: &GenOptions) -> io::Result<GenResult> {
    fs::create_dir_all(out_dir)?;
    let mut rng = Lcg::new(opts.seed ^ (opts.files as u64));

    write_project_files(out_dir)?;

    let desired_leaf_dirs = opts.files.div_ceil(50).max(1);
    let depth = if desired_leaf_dirs <= 300 { 3 } else { 4 };
    let branching = if desired_leaf_dirs <= 1 {
        1
    } else {
        (1..)
            .find(|b: &usize| b.pow(depth as u32) >= desired_leaf_dirs)
            .unwrap_or(1)
    };

    let mut nodes: Vec<DirNode> = Vec::new();
    build_tree(&mut nodes, vec!["src".to_string()], 0, depth, branching);

    let leaf_indices: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.is_leaf)
        .map(|(i, _)| i)
        .collect();

    // Distribute content files round-robin-ish (base + remainder) across leaf dirs.
    let leaf_count = leaf_indices.len();
    let base = opts.files / leaf_count;
    let rem = opts.files % leaf_count;

    let mut export_counter: u64 = 0;
    let mut files: Vec<FileMeta> = Vec::new();
    for (i, &leaf) in leaf_indices.iter().enumerate() {
        let count = base + usize::from(i < rem);
        for local in 0..count {
            let file_idx = files.len();
            let mut components = nodes[leaf].components.clone();
            components.push(format!("file{local}"));

            let export_count = 2 + rng.gen_range(4); // 2..=5
            let mut exports = Vec::with_capacity(export_count);
            for _ in 0..export_count {
                exports.push(gen_export(&mut rng, &mut export_counter));
            }
            let default_export =
                (rng.next_f64() < 0.15).then(|| gen_export_named(&mut rng, "default".to_string()));

            nodes[leaf].files.push(file_idx);
            files.push(FileMeta {
                dir: leaf,
                components,
                exports,
                default_export,
            });
        }
    }
    let content_files = files.len();

    for node in &nodes {
        fs::create_dir_all(out_dir.join(node.components.join("/")))?;
    }
    write_content_files(out_dir, &nodes, &files, &mut rng)?;
    let barrel_files = write_barrels(out_dir, &nodes)?;
    let ambient_files = write_ambient_modules(out_dir)?;

    Ok(GenResult {
        content_files,
        barrel_files,
        ambient_files,
    })
}

fn gen_export(rng: &mut Lcg, counter: &mut u64) -> ExportMeta {
    let name = format!("v{counter}");
    *counter += 1;
    gen_export_named(rng, name)
}

fn gen_export_named(rng: &mut Lcg, name: String) -> ExportMeta {
    let tag = if rng.next_f64() < 0.30 {
        Some(if rng.next_f64() < 0.6 {
            "@package"
        } else {
            "@private"
        })
    } else {
        None
    };
    ExportMeta { name, tag }
}

/// Recursively build the directory tree. `level == depth` marks a leaf.
fn build_tree(
    nodes: &mut Vec<DirNode>,
    components: Vec<String>,
    level: usize,
    depth: usize,
    branching: usize,
) -> usize {
    let idx = nodes.len();
    let is_leaf = level >= depth;
    nodes.push(DirNode {
        components: components.clone(),
        children: Vec::new(),
        files: Vec::new(),
        is_leaf,
    });
    if !is_leaf {
        for i in 0..branching {
            let mut child_components = components.clone();
            child_components.push(format!("g{i}"));
            let child_idx = build_tree(nodes, child_components, level + 1, depth, branching);
            nodes[idx].children.push(child_idx);
        }
    }
    idx
}

fn write_project_files(out_dir: &Path) -> io::Result<()> {
    fs::write(
        out_dir.join("package.json"),
        "{\n  \"name\": \"synthetic-fixture\",\n  \"version\": \"1.0.0\",\n  \"private\": true\n}\n",
    )?;
    fs::write(
        out_dir.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "baseUrl": ".",
    "paths": {
      "@app/*": ["src/*"],
      "@lib/*": ["src/*"]
    },
    "skipLibCheck": true
  }
}
"#,
    )
}

fn write_ambient_modules(out_dir: &Path) -> io::Result<usize> {
    let dir = out_dir.join("src/ambient");
    fs::create_dir_all(&dir)?;
    for (i, (module_name, exports)) in AMBIENT_MODULES.iter().enumerate() {
        let mut body = String::new();
        body.push_str(&format!("declare module \"{module_name}\" {{\n"));
        for export in *exports {
            if export.chars().next().is_some_and(char::is_uppercase) {
                body.push_str(&format!("  export const {export}: string;\n"));
            } else {
                body.push_str(&format!("  export function {export}(): void;\n"));
            }
        }
        body.push_str("}\n");
        fs::write(dir.join(format!("ambient{i}.d.ts")), body)?;
    }
    Ok(AMBIENT_MODULES.len())
}

fn write_barrels(out_dir: &Path, nodes: &[DirNode]) -> io::Result<usize> {
    for node in nodes {
        let mut body = String::new();
        if node.is_leaf {
            // `node.files` is in the same order local filenames were assigned
            // (`file0`, `file1`, ...), so the position is the local index.
            for local in 0..node.files.len() {
                body.push_str(&format!("export * from \"./file{local}\";\n"));
            }
        } else {
            for &child in &node.children {
                let child_name = nodes[child].components.last().unwrap();
                body.push_str(&format!("export * from \"./{child_name}\";\n"));
            }
        }
        let dir_path = out_dir.join(node.components.join("/"));
        fs::create_dir_all(&dir_path)?;
        fs::write(dir_path.join("index.ts"), body)?;
    }
    Ok(nodes.len())
}

fn write_content_files(
    out_dir: &Path,
    nodes: &[DirNode],
    files: &[FileMeta],
    rng: &mut Lcg,
) -> io::Result<()> {
    for (file_idx, file) in files.iter().enumerate() {
        let mut source = String::new();
        for (i, export) in file.exports.iter().enumerate() {
            write_export_decl(&mut source, export, i);
        }
        if let Some(default_export) = &file.default_export {
            if let Some(tag) = default_export.tag {
                source.push_str(&format!("/** {tag} */\n"));
            }
            source.push_str("export default function () {\n  return 0;\n}\n\n");
        }

        write_imports(&mut source, out_dir, nodes, files, file_idx, rng);

        let dir_path = out_dir.join(file.components[..file.components.len() - 1].join("/"));
        let file_name = format!("{}.ts", file.components.last().unwrap());
        fs::write(dir_path.join(file_name), source)?;
    }
    Ok(())
}

fn write_export_decl(source: &mut String, export: &ExportMeta, kind_seed: usize) {
    if let Some(tag) = export.tag {
        source.push_str(&format!("/** {tag} */\n"));
    }
    let name = &export.name;
    match kind_seed % 5 {
        0 => source.push_str(&format!("export const {name} = {kind_seed};\n\n")),
        1 => source.push_str(&format!(
            "export function {name}(): number {{\n  return {kind_seed};\n}}\n\n"
        )),
        2 => source.push_str(&format!(
            "export class {name} {{\n  value = {kind_seed};\n}}\n\n"
        )),
        3 => source.push_str(&format!(
            "export interface {name} {{\n  value: number;\n}}\n\n"
        )),
        _ => source.push_str(&format!("export type {name} = {{ value: number }};\n\n")),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_imports(
    source: &mut String,
    _out_dir: &Path,
    nodes: &[DirNode],
    files: &[FileMeta],
    file_idx: usize,
    rng: &mut Lcg,
) {
    let file = &files[file_idx];
    let dir_components = &nodes[file.dir].components;
    let import_count = 1 + rng.gen_range(5); // 1..=5
    let mut used: HashSet<String> = HashSet::new();
    let mut default_counter = 0usize;

    let mut attempts = 0;
    let mut written = 0;
    while written < import_count && attempts < import_count * 6 {
        attempts += 1;
        let roll = rng.gen_range(100);
        let same_dir_files = &nodes[file.dir].files;

        // Same-directory imports can never violate the (default) directory-based
        // access rule, so their target export is picked uniformly; cross-directory
        // paths (cross-dir/barrel/alias) are the only ones that *can* violate, so
        // they're biased toward untagged exports — with only an
        // `VIOLATION_CHANCE` probability of deliberately reaching for a
        // `@package`/`@private` one — to land "a few percent" of all imports as
        // real diagnostics (PLAN.md §8) rather than the ~20% a uniform pick would
        // produce (roughly 30% of all exports carry a tag).
        const VIOLATION_CHANCE: f64 = 0.08;

        let line = if roll < 30 && same_dir_files.len() > 1 {
            pick_sibling_import(
                rng,
                same_dir_files,
                file_idx,
                files,
                &mut used,
                &mut default_counter,
            )
        } else if roll < 70 {
            pick_cross_dir_import(
                rng,
                dir_components,
                files,
                file_idx,
                &mut used,
                &mut default_counter,
                VIOLATION_CHANCE,
            )
        } else if roll < 90 {
            pick_barrel_import(
                rng,
                dir_components,
                nodes,
                files,
                &mut used,
                VIOLATION_CHANCE,
            )
        } else if roll < 97 {
            pick_alias_import(rng, files, file_idx, &mut used, VIOLATION_CHANCE)
        } else {
            pick_ambient_import(rng, &mut used)
        };

        if let Some(line) = line {
            source.push_str(&line);
            written += 1;
        }
    }
    source.push('\n');
}

fn relative_import_path(from_dir: &[String], to: &[String]) -> String {
    let common = from_dir
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_string(), from_dir.len() - common).collect();
    parts.extend(to[common..].iter().cloned());
    let joined = parts.join("/");
    if joined.starts_with("..") {
        joined
    } else {
        format!("./{joined}")
    }
}

fn pick_sibling_import(
    rng: &mut Lcg,
    same_dir_files: &[usize],
    self_idx: usize,
    files: &[FileMeta],
    used: &mut HashSet<String>,
    default_counter: &mut usize,
) -> Option<String> {
    let candidates: Vec<usize> = same_dir_files
        .iter()
        .copied()
        .filter(|&f| f != self_idx)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let target = candidates[rng.gen_range(candidates.len())];
    let target_local = files[target].components.last().unwrap();
    let spec = format!("./{target_local}");
    // Same-directory target: never a violation regardless of tag, so no bias
    // needed — pass a neutral chance (matches the natural ~30% tag rate).
    emit_import(rng, &spec, &files[target], used, default_counter, 0.3)
}

#[allow(clippy::too_many_arguments)]
fn pick_cross_dir_import(
    rng: &mut Lcg,
    from_dir: &[String],
    files: &[FileMeta],
    self_idx: usize,
    used: &mut HashSet<String>,
    default_counter: &mut usize,
    violation_chance: f64,
) -> Option<String> {
    if files.len() <= 1 {
        return None;
    }
    let target = loop {
        let candidate = rng.gen_range(files.len());
        if candidate != self_idx {
            break candidate;
        }
    };
    let spec = relative_import_path(from_dir, &files[target].components);
    emit_import(
        rng,
        &spec,
        &files[target],
        used,
        default_counter,
        violation_chance,
    )
}

fn pick_barrel_import(
    rng: &mut Lcg,
    from_dir: &[String],
    nodes: &[DirNode],
    files: &[FileMeta],
    used: &mut HashSet<String>,
    violation_chance: f64,
) -> Option<String> {
    if nodes.len() <= 1 {
        return None;
    }
    let target_node = loop {
        let candidate = rng.gen_range(nodes.len());
        if nodes[candidate].components != from_dir {
            break candidate;
        }
    };
    let names = barrel_sample_names(nodes, files, target_node);
    if names.is_empty() {
        return None;
    }
    let pick = biased_pick(rng, &names, |(_, tag)| *tag, violation_chance)?;
    let name = &names[pick].0;
    let key = format!("barrel:{}:{name}", nodes[target_node].components.join("/"));
    if !used.insert(key) {
        return None;
    }
    let spec = relative_import_path(from_dir, &nodes[target_node].components);
    Some(format!("import {{ {name} }} from \"{spec}\";\n"))
}

/// A handful of export names (with their JSDoc access tag) reachable via the
/// `export *` barrel chain from `node_idx`'s subtree — cheap to compute by always
/// walking the first child down to a leaf, rather than enumerating every file in a
/// potentially huge subtree.
fn barrel_sample_names(
    nodes: &[DirNode],
    files: &[FileMeta],
    node_idx: usize,
) -> Vec<(String, Option<&'static str>)> {
    let mut idx = node_idx;
    while !nodes[idx].is_leaf {
        match nodes[idx].children.first() {
            Some(&child) => idx = child,
            None => return Vec::new(),
        }
    }
    nodes[idx]
        .files
        .iter()
        .flat_map(|&f| files[f].exports.iter().map(|e| (e.name.clone(), e.tag)))
        .take(8)
        .collect()
}

fn pick_alias_import(
    rng: &mut Lcg,
    files: &[FileMeta],
    self_idx: usize,
    used: &mut HashSet<String>,
    violation_chance: f64,
) -> Option<String> {
    if files.len() <= 1 {
        return None;
    }
    let target = loop {
        let candidate = rng.gen_range(files.len());
        if candidate != self_idx {
            break candidate;
        }
    };
    let target_file = &files[target];
    let pick = biased_pick(rng, &target_file.exports, |e| e.tag, violation_chance)?;
    let export = &target_file.exports[pick];
    if !used.insert(export.name.clone()) {
        return None;
    }
    // `components` is `["src", ...]`; the alias maps to everything after `src/`.
    let alias_path = target_file.components[1..].join("/");
    Some(format!(
        "import {{ {} }} from \"@app/{alias_path}\";\n",
        export.name
    ))
}

/// Pick an index into `items`, biased toward entries whose `tag_of` is `None`
/// (untagged): only reach for a tagged entry with probability `violation_chance`
/// (and only when at least one untagged entry doesn't exist do we fall back to a
/// tagged one unconditionally). Returns `None` for an empty slice.
fn biased_pick<T>(
    rng: &mut Lcg,
    items: &[T],
    tag_of: impl Fn(&T) -> Option<&'static str>,
    violation_chance: f64,
) -> Option<usize> {
    let mut untagged = Vec::new();
    let mut tagged = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if tag_of(item).is_some() {
            tagged.push(i);
        } else {
            untagged.push(i);
        }
    }
    if untagged.is_empty() && tagged.is_empty() {
        return None;
    }
    let want_tagged =
        !tagged.is_empty() && (untagged.is_empty() || rng.next_f64() < violation_chance);
    let pool = if want_tagged { &tagged } else { &untagged };
    Some(pool[rng.gen_range(pool.len())])
}

fn pick_ambient_import(rng: &mut Lcg, used: &mut HashSet<String>) -> Option<String> {
    let (module_name, exports) = &AMBIENT_MODULES[rng.gen_range(AMBIENT_MODULES.len())];
    let export = exports[rng.gen_range(exports.len())];
    let key = format!("ambient:{module_name}:{export}");
    if !used.insert(key) {
        return None;
    }
    Some(format!("import {{ {export} }} from \"{module_name}\";\n"))
}

/// Shared by the sibling/cross-dir paths: pick one of `target`'s exports (or its
/// default export, itself taggable) via [`biased_pick`] and emit a matching import
/// line, guarding against picking the same binding twice in one file (which would
/// be a duplicate-declaration parse error).
fn emit_import(
    rng: &mut Lcg,
    spec: &str,
    target: &FileMeta,
    used: &mut HashSet<String>,
    default_counter: &mut usize,
    violation_chance: f64,
) -> Option<String> {
    // `None` represents the default export slot; `Some(i)` a named export index.
    let mut candidates: Vec<(Option<usize>, Option<&'static str>)> = target
        .exports
        .iter()
        .enumerate()
        .map(|(i, e)| (Some(i), e.tag))
        .collect();
    if let Some(default_export) = &target.default_export {
        candidates.push((None, default_export.tag));
    }
    let pick = biased_pick(rng, &candidates, |(_, tag)| *tag, violation_chance)?;

    match candidates[pick].0 {
        None => {
            let key = format!("default:{spec}");
            if !used.insert(key) {
                return None;
            }
            let local = format!("defaultImport{default_counter}");
            *default_counter += 1;
            Some(format!("import {local} from \"{spec}\";\n"))
        }
        Some(idx) => {
            let export = &target.exports[idx];
            if !used.insert(export.name.clone()) {
                return None;
            }
            Some(format!("import {{ {} }} from \"{spec}\";\n", export.name))
        }
    }
}
