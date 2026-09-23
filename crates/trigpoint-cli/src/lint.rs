//! `trigp lint`: runs whichever determinism checks the workspace declares.
//!
//! Target detection is config-driven. A `triglint.toml` carrying a `[python]`
//! section means there are Python sources to check; a `Cargo.toml` at or above
//! the directory declaring dylint libraries means there is a Rust target.
//! Whichever applies runs; both run when both apply. `--rust` / `--python`
//! override the detection explicitly.
//!
//! The two paths could not be less alike: the Rust one shells out to
//! cargo-dylint under a pinned nightly with MIR encoding arranged, the Python
//! one is a library call. Exit codes are unified — any deny-level finding from
//! either fails the run.

pub mod dylint;
pub mod python;

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use thiserror::Error;

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
    /// Check only the Rust target (cargo-dylint).
    #[arg(long)]
    pub rust: bool,
    /// Check only the Python target.
    #[arg(long)]
    pub python: bool,
    /// Extra arguments passed through to `cargo check` (e.g. --features).
    #[arg(last = true)]
    pub cargo_args: Vec<OsString>,
}

#[derive(Debug, Error)]
pub enum LintError {
    #[error("failed to resolve current directory: {0}")]
    CurrentDir(#[source] std::io::Error),
    #[error(
        "cargo-dylint is not installed or not working (install with `cargo install cargo-dylint dylint-link`)"
    )]
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
    #[error(transparent)]
    PythonConfig(#[from] trigpoint_pylint::config::ConfigError),
    #[error(transparent)]
    Python(#[from] trigpoint_pylint::PylintError),
}

/// Which checks this invocation will run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub rust: bool,
    pub python: bool,
}

impl Targets {
    pub fn none(self) -> bool {
        !self.rust && !self.python
    }

    /// Explicit flags win; otherwise each target is enabled by the evidence
    /// that it exists.
    pub fn detect(rust_flag: bool, python_flag: bool, has_dylint: bool, has_python: bool) -> Self {
        if rust_flag || python_flag {
            return Self {
                rust: rust_flag,
                python: python_flag,
            };
        }
        Self {
            rust: has_dylint,
            python: has_python,
        }
    }
}

pub fn run(args: LintArgs) -> Result<ExitCode, LintError> {
    let dir = match args.dir {
        Some(dir) => dir,
        None => env::current_dir().map_err(LintError::CurrentDir)?,
    };

    let config = find_triglint_toml(&dir);
    if config.is_none() {
        eprintln!(
            "trigp: warning: no triglint.toml found walking up from {}; triglint will have nothing to check",
            dir.display()
        );
    }
    let has_python = match config.as_deref() {
        Some(path) => trigpoint_pylint::config::parse_file(path)?.is_some(),
        None => false,
    };
    let targets = Targets::detect(
        args.rust,
        args.python,
        dylint::has_metadata(&dir),
        has_python,
    );

    if targets.none() {
        eprintln!(
            "trigp: no lint target detected in {}: add [workspace.metadata.dylint] for the Rust lint, or a [python] section to triglint.toml for the Python lint",
            dir.display()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let mut python_failed = false;
    if targets.python {
        match config.as_deref() {
            Some(path) => python_failed = python::run(path)?.failed(),
            None => eprintln!("trigp: --python given but no triglint.toml was found"),
        }
    }

    let mut rust_code = 0;
    if targets.rust {
        rust_code = dylint::run(&dir, args.fresh, !args.no_deps_mir, &args.cargo_args)?;
    }

    Ok(exit_code(rust_code, python_failed))
}

/// A failure from either target fails the run; the Rust exit code is preserved
/// when it is the one that failed.
fn exit_code(rust_code: i32, python_failed: bool) -> ExitCode {
    if rust_code != 0 {
        return ExitCode::from(u8::try_from(rust_code).unwrap_or(1));
    }
    if python_failed {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn detection_follows_the_evidence() {
        assert_eq!(
            Targets::detect(false, false, true, false),
            Targets {
                rust: true,
                python: false
            }
        );
        assert_eq!(
            Targets::detect(false, false, false, true),
            Targets {
                rust: false,
                python: true
            }
        );
        assert_eq!(
            Targets::detect(false, false, true, true),
            Targets {
                rust: true,
                python: true
            }
        );
        assert!(Targets::detect(false, false, false, false).none());
    }

    #[test]
    fn explicit_flags_override_detection() {
        // `--python` in a dylint workspace must not also run cargo-dylint.
        assert_eq!(
            Targets::detect(false, true, true, true),
            Targets {
                rust: false,
                python: true
            }
        );
        // `--rust` runs the Rust lint even where no dylint metadata was found.
        assert_eq!(
            Targets::detect(true, false, false, true),
            Targets {
                rust: true,
                python: false
            }
        );
    }

    #[test]
    fn either_target_failing_fails_the_run() {
        assert_eq!(
            format!("{:?}", exit_code(0, false)),
            format!("{:?}", ExitCode::SUCCESS)
        );
        assert_eq!(
            format!("{:?}", exit_code(0, true)),
            format!("{:?}", ExitCode::FAILURE)
        );
        assert_eq!(
            format!("{:?}", exit_code(101, false)),
            format!("{:?}", ExitCode::from(101u8))
        );
        assert_eq!(
            format!("{:?}", exit_code(101, true)),
            format!("{:?}", ExitCode::from(101u8))
        );
    }
}
