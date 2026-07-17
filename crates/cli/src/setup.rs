//! Project setup: turning CLI flags + config-file discovery into a `LintConfig`,
//! project root, roots to walk, `tsconfig` path, and self-reference mode (PLAN-v1.md
//! §5, D7). Factored out of `main.rs`'s `lint()` (M5) so watch mode's `ConfigChanged`
//! reload path (`crates/cli/src/watch.rs`, M6 brief D-W1) can redo exactly the same
//! computation without duplicating it.

use std::fmt;
use std::path::{Path, PathBuf};

use import_lint::rule::SelfRefOpt;
use import_lint::{ConfigError, LintConfig, SelfReferenceMode, find_config};

use crate::runner::RunnerOptions;

/// A loaded config plus the project root it implies (D7: the directory containing
/// the config file, or the caller's cwd if there's no config file at all).
pub struct LoadedConfig {
    pub config: LintConfig,
    pub project_root: PathBuf,
    /// The config file that was loaded, if any (`None` means defaults, no config
    /// file found).
    pub config_path: Option<PathBuf>,
}

/// Everything that can go wrong resolving a config: an explicit `--config` path that
/// doesn't exist, or a discovered/explicit file that fails to parse.
#[derive(Debug)]
pub enum ConfigLoadError {
    ExplicitConfigMissing(PathBuf),
    Parse(ConfigError),
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigLoadError::ExplicitConfigMissing(path) => {
                write!(f, "--config {}: no such file", path.display())
            }
            ConfigLoadError::Parse(err) => write!(f, "failed to load config: {err}"),
        }
    }
}

impl std::error::Error for ConfigLoadError {}

/// Resolve the config file to load: `explicit_config` if given (must exist — an
/// explicit path that doesn't exist is a usage error, not "use defaults"), else
/// discovery from `cwd` upward (D7). Load it (or fall back to `LintConfig::default()`
/// with `project_root = cwd` when no config file exists at all).
pub fn load_config(
    explicit_config: Option<&Path>,
    cwd: &Path,
) -> Result<LoadedConfig, ConfigLoadError> {
    let config_path = match explicit_config {
        Some(explicit) => {
            if !explicit.is_file() {
                return Err(ConfigLoadError::ExplicitConfigMissing(
                    explicit.to_path_buf(),
                ));
            }
            Some(explicit.to_path_buf())
        }
        None => find_config(cwd),
    };

    match config_path {
        Some(path) => match LintConfig::load(&path) {
            Ok(config) => {
                let project_root = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| cwd.to_path_buf());
                Ok(LoadedConfig {
                    config,
                    project_root,
                    config_path: Some(path),
                })
            }
            Err(err) => Err(ConfigLoadError::Parse(err)),
        },
        None => Ok(LoadedConfig {
            config: LintConfig::default(),
            project_root: cwd.to_path_buf(),
            config_path: None,
        }),
    }
}

/// The roots to walk: `cli_paths` if non-empty (an explicit CLI override always
/// wins), else `config.include` resolved relative to `project_root`.
pub fn compute_roots(
    cli_paths: &[PathBuf],
    config: &LintConfig,
    project_root: &Path,
) -> Vec<PathBuf> {
    if cli_paths.is_empty() {
        config
            .include
            .iter()
            .map(|root| project_root.join(root))
            .collect()
    } else {
        cli_paths.to_vec()
    }
}

/// The `tsconfig.json` to feed the resolver: `--tsconfig` if given, else
/// `config.tsconfig` resolved relative to `project_root`, else
/// `<project_root>/tsconfig.json` if that file exists (`RunnerOptions::default_tsconfig`).
pub fn compute_tsconfig(
    cli_tsconfig: Option<&Path>,
    config: &LintConfig,
    project_root: &Path,
) -> Option<PathBuf> {
    cli_tsconfig
        .map(Path::to_path_buf)
        .or_else(|| config.tsconfig.as_ref().map(|path| project_root.join(path)))
        .or_else(|| RunnerOptions::default_tsconfig(project_root))
}

/// `config`'s `treatSelfReferenceAs` option (spec §4.6), translated to core's
/// `SelfReferenceMode`.
pub fn compute_self_reference_mode(config: &LintConfig) -> SelfReferenceMode {
    match config.rules.package_access.options.treat_self_reference_as {
        SelfRefOpt::Internal => SelfReferenceMode::Internal,
        SelfRefOpt::External => SelfReferenceMode::External,
    }
}
