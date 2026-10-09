//! `trigp spec check`: the stopgap invariant/evidence validator.
//!
//! Loads `spec/invariants/*.toml`, resolves test pointers against what
//! `cargo test -- --list` reports, checks commit-pinned reviews for
//! staleness via git, and prints the gap report. Structural errors fail
//! the run; gaps fail it only under `--strict`.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use thiserror::Error;
use trigpoint_core::check::{self, CheckContext};
use trigpoint_core::git::GitOracle;
use trigpoint_core::report;
use trigpoint_core::resolve::cargo::{self, CargoError};
use trigpoint_core::spec::load::{self, LoadError};

#[derive(clap::Args)]
pub struct SpecArgs {
    #[command(subcommand)]
    pub action: SpecAction,
}

#[derive(clap::Subcommand)]
pub enum SpecAction {
    /// Report, per invariant, which required evidence kinds are missing,
    /// which pointers do not resolve, and what has not been reviewed.
    Check(CheckArgs),
    /// Compare committed invariant specs and optionally update a GitHub PR comment.
    Diff(crate::spec_diff::DiffArgs),
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// Workspace directory (defaults to the current directory).
    #[arg(long, short = 'C')]
    pub dir: Option<PathBuf>,
    /// Spec directory, relative to the workspace directory.
    #[arg(long, default_value = "spec/invariants")]
    pub spec_dir: PathBuf,
    /// Also fail on gaps (missing or unreviewed evidence, stale reviews),
    /// not only on structural errors.
    #[arg(long)]
    pub strict: bool,
    /// Skip building and listing the workspace's tests; test pointers are
    /// then taken on faith.
    #[arg(long)]
    pub no_resolve: bool,
}

#[derive(Debug, Error)]
pub enum SpecError {
    #[error(transparent)]
    Diff(#[from] crate::spec_diff::DiffError),
    #[error("failed to resolve current directory: {0}")]
    CurrentDir(#[source] std::io::Error),
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Cargo(#[from] CargoError),
}

pub fn run(args: SpecArgs) -> Result<ExitCode, SpecError> {
    match args.action {
        SpecAction::Check(args) => run_check(args),
        SpecAction::Diff(args) => Ok(crate::spec_diff::run(args)?),
    }
}

fn run_check(args: CheckArgs) -> Result<ExitCode, SpecError> {
    let dir = match args.dir {
        Some(dir) => dir,
        None => env::current_dir().map_err(SpecError::CurrentDir)?,
    };
    let spec_dir = dir.join(&args.spec_dir);
    let loaded = load::load_dir(&spec_dir)?;

    let tests = if args.no_resolve {
        None
    } else {
        eprintln!("trigp: building tests to resolve evidence pointers...");
        Some(cargo::collect(&dir)?)
    };
    let oracle = GitOracle::new(&dir);
    let ctx = CheckContext {
        tests: tests.as_ref(),
        oracle: Some(&oracle),
    };
    let report = check::check(loaded, &ctx);
    print!("{}", report::render(&report));

    let failed = report.has_errors() || (args.strict && report.has_gaps());
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
