mod lint;
mod spec;
mod spec_diff;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "trigp",
    version,
    about = "Spec-driven development harness: invariants, evidence, and determinism linting"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the determinism lints a workspace declares: triglint via
    /// cargo-dylint for Rust (with dependency MIR encoding set up so
    /// cross-crate analysis works) and trigpoint-pylint for Python.
    Lint(lint::LintArgs),
    /// Validate invariant specs or diff a branch against its base.
    Spec(spec::SpecArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Lint(args) => match lint::run(args) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("trigp: error: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Spec(args) => match spec::run(args) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("trigp: error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}
