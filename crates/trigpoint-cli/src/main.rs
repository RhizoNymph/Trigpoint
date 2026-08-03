mod lint;

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
    /// Run triglint over a workspace via cargo-dylint, with dependency MIR
    /// encoding set up so cross-crate analysis works.
    Lint(lint::LintArgs),
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
    }
}
