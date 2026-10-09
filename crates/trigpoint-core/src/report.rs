//! Rendering a check report as text.

use std::fmt::Write as _;

use crate::check::{Report, Resolution, Severity};

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "spec: {} invariant(s) in {}",
        report.invariants.len(),
        report.spec_dir.display()
    );
    for problem in &report.problems {
        let _ = writeln!(out, "  error  {problem}");
    }
    for invariant in &report.invariants {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}  ({})", invariant.id, invariant.path.display());
        if invariant.findings.is_empty() {
            let _ = writeln!(
                out,
                "  ok     all required evidence present, resolved, and reviewed"
            );
        }
        for finding in &invariant.findings {
            let _ = writeln!(out, "  {:<6} {finding}", finding.severity().to_string());
        }
    }
    let _ = writeln!(out);
    let _ = write!(
        out,
        "summary: {} error(s), {} gap(s), {} info",
        report.count(Severity::Error),
        report.count(Severity::Gap),
        report.count(Severity::Info)
    );
    match report.resolution {
        Resolution::Resolved { tests } => {
            let _ = write!(out, "; test pointers resolved against {tests} test(s)");
        }
        Resolution::Skipped => {
            let _ = write!(out, "; test pointers not resolved (--no-resolve)");
        }
    }
    let _ = writeln!(out);
    out
}
