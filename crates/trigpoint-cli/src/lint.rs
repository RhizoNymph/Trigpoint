//! `trigp lint`: orchestrates cargo-dylint so users don't have to know the
//! environment folklore (DYLINT_RUSTFLAGS vs RUSTFLAGS, cache busting when
//! MIR flags change).

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use thiserror::Error;

/// Flag that makes dependency bodies traversable by triglint. Passed via
/// DYLINT_RUSTFLAGS (never RUSTFLAGS, which leaks into cargo-dylint's
/// stable-toolchain probes and breaks library discovery).
pub const ALWAYS_ENCODE_MIR: &str = "-Zalways-encode-mir";

#[derive(clap::Args)]
pub struct LintArgs {
    /// Workspace directory to lint (defaults to the current directory).
    #[arg(long, short = 'C')]
    pub dir: Option<PathBuf>,
    /// Clear dylint's analysis cache first. Needed when toggling MIR flags:
    /// DYLINT_RUSTFLAGS changes do not invalidate cargo's fingerprints.
    #[arg(long)]
    pub fresh: bool,
    /// Skip -Zalways-encode-mir. Dependency bodies become opaque and are
    /// reported as sim_unresolved warnings instead of being traversed.
    #[arg(long)]
    pub no_deps_mir: bool,
    /// Extra arguments passed through to `cargo check` (e.g. --features).
    #[arg(last = true)]
    pub cargo_args: Vec<OsString>,
}

#[derive(Debug, Error)]
pub enum LintError {
    #[error("failed to resolve current directory: {0}")]
    CurrentDir(#[source] std::io::Error),
    #[error("cargo-dylint is not installed or not working (install with `cargo install cargo-dylint dylint-link`)")]
    CargoDylintMissing,
    #[error("failed to run {what}: {source}")]
    Spawn {
        what: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("cargo metadata failed: {stderr}")]
    Metadata { stderr: String },
    #[error("could not parse cargo metadata: {0}")]
    MetadataParse(#[from] serde_json::Error),
    #[error("failed to clear dylint cache at {path}: {source}")]
    ClearCache {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn run(args: LintArgs) -> Result<ExitCode, LintError> {
    let dir = match args.dir {
        Some(dir) => dir,
        None => env::current_dir().map_err(LintError::CurrentDir)?,
    };

    if find_triglint_toml(&dir).is_none() {
        eprintln!(
            "trigp: warning: no triglint.toml found walking up from {}; triglint will have nothing to check",
            dir.display()
        );
    }

    ensure_cargo_dylint(&dir)?;

    if args.fresh {
        clear_analysis_cache(&dir)?;
    }

    let flags = merged_dylint_rustflags(
        env::var("DYLINT_RUSTFLAGS").ok().as_deref(),
        !args.no_deps_mir,
    );

    let mut command = Command::new("cargo");
    command.arg("dylint").arg("--all").current_dir(&dir);
    if let Some(flags) = flags {
        command.env("DYLINT_RUSTFLAGS", flags);
    }
    if !args.cargo_args.is_empty() {
        command.arg("--");
        command.args(&args.cargo_args);
    }
    let status = command.status().map_err(|source| LintError::Spawn {
        what: "cargo dylint",
        source,
    })?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}

/// Appends -Zalways-encode-mir to any user-provided DYLINT_RUSTFLAGS,
/// without duplicating it. `None` means "leave the variable unset".
pub fn merged_dylint_rustflags(existing: Option<&str>, deps_mir: bool) -> Option<String> {
    if !deps_mir {
        return existing.map(str::to_owned);
    }
    match existing {
        None => Some(ALWAYS_ENCODE_MIR.to_owned()),
        Some(flags) if flags.split_whitespace().any(|f| f == ALWAYS_ENCODE_MIR) => {
            Some(flags.to_owned())
        }
        Some(flags) => Some(format!("{flags} {ALWAYS_ENCODE_MIR}")),
    }
}

/// Walks up from `start` looking for triglint.toml, mirroring triglint's own
/// discovery.
pub fn find_triglint_toml(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join("triglint.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

fn ensure_cargo_dylint(dir: &Path) -> Result<(), LintError> {
    let output = Command::new("cargo")
        .args(["dylint", "--version"])
        .current_dir(dir)
        .output()
        .map_err(|source| LintError::Spawn {
            what: "cargo dylint --version",
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(LintError::CargoDylintMissing)
    }
}

/// Removes dylint's per-toolchain analysis target dir (not the built lint
/// libraries), forcing recompilation of the analyzed workspace.
fn clear_analysis_cache(dir: &Path) -> Result<(), LintError> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(dir)
        .output()
        .map_err(|source| LintError::Spawn {
            what: "cargo metadata",
            source,
        })?;
    if !output.status.success() {
        return Err(LintError::Metadata {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let Some(target_dir) = metadata["target_directory"].as_str() else {
        return Err(LintError::Metadata {
            stderr: "metadata has no target_directory".to_owned(),
        });
    };
    let cache = Path::new(target_dir).join("dylint").join("target");
    if cache.exists() {
        fs::remove_dir_all(&cache).map_err(|source| LintError::ClearCache {
            path: cache.clone(),
            source,
        })?;
        eprintln!("trigp: cleared dylint analysis cache at {}", cache.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_flag_into_empty_env() {
        assert_eq!(
            merged_dylint_rustflags(None, true).as_deref(),
            Some(ALWAYS_ENCODE_MIR)
        );
    }

    #[test]
    fn appends_flag_to_existing_flags() {
        assert_eq!(
            merged_dylint_rustflags(Some("-Zthreads=2"), true).as_deref(),
            Some("-Zthreads=2 -Zalways-encode-mir")
        );
    }

    #[test]
    fn does_not_duplicate_flag() {
        assert_eq!(
            merged_dylint_rustflags(Some("-Zalways-encode-mir"), true).as_deref(),
            Some(ALWAYS_ENCODE_MIR)
        );
    }

    #[test]
    fn no_deps_mir_leaves_env_untouched() {
        assert_eq!(merged_dylint_rustflags(None, false), None);
        assert_eq!(
            merged_dylint_rustflags(Some("-Zthreads=2"), false).as_deref(),
            Some("-Zthreads=2")
        );
    }

    #[test]
    fn finds_config_walking_up() {
        let base = env::temp_dir().join("trigp-test-find-config");
        let nested = base.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(base.join("triglint.toml"), "[sim]\nroots = []\n").unwrap();
        assert_eq!(
            find_triglint_toml(&nested),
            Some(base.join("triglint.toml"))
        );
        fs::remove_dir_all(&base).unwrap();
    }
}
