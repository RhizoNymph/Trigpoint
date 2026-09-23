//! Golden-file fixture harness.
//!
//! Two corpora, mirroring `triglint/ui`:
//!
//! * `fixtures/prod/*.py` — one flat package analyzed in a single run; each
//!   file has a `<stem>.expected` listing the diagnostics for that file.
//! * `fixtures/sim/<case>/` — a package tree with its own `triglint.toml` and
//!   one `expected.txt` for the whole case.
//!
//! Goldens are written in [`Diagnostic::summary`] form — one stable line per
//! finding — so they can be read and written by hand. Two additional goldens
//! under `fixtures/render/` pin the `annotate-snippets` rendering itself.
//!
//! Set `TRIGPYLINT_BLESS=1` to rewrite the goldens after a deliberate change.

use std::fs;
use std::path::{Path, PathBuf};

use trigpoint_pylint::Analysis;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn blessing() -> bool {
    std::env::var("TRIGPYLINT_BLESS").is_ok_and(|v| v == "1")
}

fn analyze(dir: &Path) -> Analysis {
    let config_path = dir.join("triglint.toml");
    let python = trigpoint_pylint::config::parse_file(&config_path)
        .expect("fixture config should parse")
        .expect("fixture config should have a [python] section");
    trigpoint_pylint::analyze(dir, python).expect("fixture analysis should succeed")
}

/// Compares `actual` against the golden at `expected_path`, normalising to a
/// trailing newline so an empty expectation is an empty file.
fn compare(label: &str, expected_path: &Path, actual: &str) {
    let actual = if actual.is_empty() {
        String::new()
    } else {
        format!("{}\n", actual.trim_end_matches('\n'))
    };
    if blessing() {
        fs::write(expected_path, &actual).expect("should write golden");
        return;
    }
    let expected = fs::read_to_string(expected_path).unwrap_or_else(|error| {
        panic!(
            "missing golden {} for {label}: {error}",
            expected_path.display()
        )
    });
    assert_eq!(
        expected,
        actual,
        "\n{label}: diagnostics do not match {}\n--- expected ---\n{expected}--- actual ---\n{actual}",
        expected_path.display()
    );
}

#[test]
fn prod_fixtures_match_goldens() {
    let dir = fixtures().join("prod");
    let analysis = analyze(&dir);

    let mut cases: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("prod fixtures should exist")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "py"))
        .collect();
    cases.sort();
    assert!(cases.len() >= 16, "expected the full prod corpus");

    for case in cases {
        let file_name = case
            .file_name()
            .and_then(|n| n.to_str())
            .expect("fixture file name");
        let actual = analysis
            .for_file(file_name)
            .map(trigpoint_pylint::diagnostics::Diagnostic::summary)
            .collect::<Vec<_>>()
            .join("\n");
        compare(file_name, &case.with_extension("expected"), &actual);
    }

    // Every diagnostic must belong to a fixture file; nothing may be emitted
    // for a file the corpus does not know about.
    for diagnostic in &analysis.diagnostics {
        assert!(
            dir.join(&diagnostic.display_path).is_file(),
            "diagnostic for unknown file {}",
            diagnostic.display_path
        );
    }
}

#[test]
fn sim_fixtures_match_goldens() {
    let dir = fixtures().join("sim");
    let mut cases: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("sim fixtures should exist")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    assert!(cases.len() >= 7, "expected the full sim corpus");

    for case in cases {
        let label = case
            .file_name()
            .and_then(|n| n.to_str())
            .expect("case name")
            .to_owned();
        let analysis = analyze(&case);
        let actual = analysis.summaries().join("\n");
        compare(&label, &case.join("expected.txt"), &actual);
    }
}

#[test]
fn clean_fixtures_produce_no_denials() {
    let analysis = analyze(&fixtures().join("sim").join("sim_clean"));
    assert!(!analysis.has_denials());
    assert_eq!(analysis.diagnostics.len(), 0);
    // The whole package is in scope: harness, package `__init__`, and engine.
    assert_eq!(analysis.sim_modules, 3);
}

#[test]
fn violations_set_the_deny_flag() {
    let analysis = analyze(&fixtures().join("sim").join("sim_transitive"));
    assert!(analysis.has_denials());
    assert_eq!(analysis.denials(), 1);
}

#[test]
fn rendered_prod_diagnostic_matches_golden() {
    let dir = fixtures().join("prod");
    let analysis = analyze(&dir);
    let source = fs::read_to_string(dir.join("direct_call.py")).expect("fixture source");
    let rendered = analysis
        .for_file("direct_call.py")
        .map(|d| d.render(&source))
        .collect::<Vec<_>>()
        .join("\n");
    compare(
        "direct_call.py rendering",
        &fixtures().join("render").join("prod_direct_call.expected"),
        &rendered,
    );
}

#[test]
fn rendered_sim_diagnostic_matches_golden() {
    let case = fixtures().join("sim").join("sim_transitive");
    let analysis = analyze(&case);
    compare(
        "sim_transitive rendering",
        &fixtures().join("render").join("sim_transitive.expected"),
        analysis.render().trim_end_matches('\n'),
    );
}
