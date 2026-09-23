//! The Python target: plain library code, no subprocess and no nightly.
//!
//! `trigpoint-pylint` parses the sources itself, so unlike the Rust path there
//! is no toolchain to arrange — the whole driver is "load the config, analyze,
//! print, decide the exit code".

use std::path::Path;

use trigpoint_pylint::Analysis;

use super::LintError;

/// Outcome of the Python analysis for exit-code purposes.
pub enum Outcome {
    /// No `[python]` section: nothing to check.
    Inert,
    Checked {
        denials: usize,
    },
}

impl Outcome {
    pub fn failed(&self) -> bool {
        matches!(self, Outcome::Checked { denials } if *denials > 0)
    }
}

/// Runs the Python analysis over the project whose config lives at
/// `config_path`, printing diagnostics to stderr.
pub fn run(config_path: &Path) -> Result<Outcome, LintError> {
    let Some(config) = trigpoint_pylint::config::parse_file(config_path)? else {
        return Ok(Outcome::Inert);
    };
    let root = config_path.parent().unwrap_or(Path::new("."));
    let analysis = trigpoint_pylint::analyze(root, config)?;
    report(&analysis);
    Ok(Outcome::Checked {
        denials: analysis.denials(),
    })
}

fn report(analysis: &Analysis) {
    let rendered = analysis.render();
    if !rendered.is_empty() {
        eprint!("{rendered}");
    }
    let denials = analysis.denials();
    let warnings = analysis.diagnostics.len() - denials;
    eprintln!(
        "trigp: python: {} module(s) checked, {} in simulation scope, {denials} error(s), {warnings} warning(s)",
        analysis.modules_collected, analysis.sim_modules
    );
}
