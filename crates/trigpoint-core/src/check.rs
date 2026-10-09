//! The spec check: what each invariant owes, what it has, and whether what
//! it has is real and reviewed.
//!
//! Pure over its inputs: the loaded spec, an optional test index, and an
//! optional change oracle. Findings carry a severity so callers can decide
//! what fails a run — structural errors always do, gaps only under
//! `--strict`.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::git::ChangeOracle;
use crate::resolve::{TestIndex, TestSet, pointer_target};
use crate::spec::load::{Loaded, SpecProblem};
use crate::spec::{EvidenceKind, PointerScheme, Review, ReviewEntry, ReviewState};

/// Lint providers whose evidence statements the check accepts by name.
pub const LINT_PROVIDERS: &[&str] = &["triglint", "trigpoint-pylint"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The spec is wrong: unresolvable pointers, reviews of nothing, bad
    /// commits.
    Error,
    /// The spec is honest but incomplete: missing or unreviewed evidence.
    Gap,
    /// Worth knowing, not actionable.
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Gap => "gap",
            Severity::Info => "info",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reviewer {
    Agent,
    Human,
}

impl fmt::Display for Reviewer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Reviewer::Agent => "agent",
            Reviewer::Human => "human",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `requires` is empty: nothing to check.
    NothingRequired,
    /// No `[evidence]` table at all.
    NoEvidence { required: Vec<EvidenceKind> },
    /// A required kind with no entry in `[evidence]`.
    MissingEvidence(EvidenceKind),
    /// An `[evidence]` kind that `requires` does not list.
    ExtraEvidence(EvidenceKind),
    /// A pointer that names nothing that exists.
    UnresolvedPointer {
        kind: EvidenceKind,
        pointer: String,
        scheme: PointerScheme,
    },
    /// A `[review]` entry for a kind with no evidence.
    ReviewWithoutEvidence(EvidenceKind),
    /// Evidence nobody of this role has assessed.
    Unreviewed { kind: EvidenceKind, by: Reviewer },
    /// A review pinned to a commit the repository does not have.
    UnknownCommit {
        kind: EvidenceKind,
        by: Reviewer,
        commit: String,
    },
    /// A commit-pinned review whose sources have changed since.
    StaleReview {
        kind: EvidenceKind,
        by: Reviewer,
        commit: String,
    },
    /// A commit-pinned review the check could not assess (no oracle, or no
    /// source paths known for the evidence).
    StalenessUnknown {
        kind: EvidenceKind,
        by: Reviewer,
        commit: String,
    },
    /// The oracle itself failed.
    OracleFailed { message: String },
}

impl Finding {
    pub fn severity(&self) -> Severity {
        match self {
            Finding::NothingRequired
            | Finding::ExtraEvidence(_)
            | Finding::StalenessUnknown { .. } => Severity::Info,
            Finding::NoEvidence { .. }
            | Finding::MissingEvidence(_)
            | Finding::Unreviewed { .. }
            | Finding::StaleReview { .. } => Severity::Gap,
            Finding::UnresolvedPointer { .. }
            | Finding::ReviewWithoutEvidence(_)
            | Finding::UnknownCommit { .. }
            | Finding::OracleFailed { .. } => Severity::Error,
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Finding::NothingRequired => f.write_str("requires no evidence"),
            Finding::NoEvidence { required } => {
                write!(f, "no evidence section; required: {}", join_kinds(required))
            }
            Finding::MissingEvidence(kind) => write!(f, "missing {kind} evidence"),
            Finding::ExtraEvidence(kind) => {
                write!(f, "{kind} evidence present but not in requires")
            }
            Finding::UnresolvedPointer {
                kind,
                pointer,
                scheme,
            } => write!(
                f,
                "{kind} evidence points at {} `{pointer}`, which does not exist",
                scheme.describe()
            ),
            Finding::ReviewWithoutEvidence(kind) => {
                write!(f, "review of {kind} evidence that is not declared")
            }
            Finding::Unreviewed { kind, by } => write!(f, "{kind} evidence not reviewed by {by}"),
            Finding::UnknownCommit { kind, by, commit } => write!(
                f,
                "{by} review of {kind} evidence pins commit `{commit}`, which is not in this repository"
            ),
            Finding::StaleReview { kind, by, commit } => write!(
                f,
                "{by} review of {kind} evidence is stale: sources changed since `{commit}`"
            ),
            Finding::StalenessUnknown { kind, by, commit } => write!(
                f,
                "{by} review of {kind} evidence pinned at `{commit}` could not be checked for staleness"
            ),
            Finding::OracleFailed { message } => write!(f, "could not query git: {message}"),
        }
    }
}

fn join_kinds(kinds: &[EvidenceKind]) -> String {
    kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug)]
pub struct InvariantReport {
    pub id: String,
    pub path: PathBuf,
    pub findings: Vec<Finding>,
}

/// Whether test pointers were resolved against a real test set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Resolved { tests: usize },
    Skipped,
}

#[derive(Debug)]
pub struct Report {
    pub spec_dir: PathBuf,
    pub problems: Vec<SpecProblem>,
    pub invariants: Vec<InvariantReport>,
    pub resolution: Resolution,
}

impl Report {
    pub fn count(&self, severity: Severity) -> usize {
        let from_findings = self
            .invariants
            .iter()
            .flat_map(|i| i.findings.iter())
            .filter(|f| f.severity() == severity)
            .count();
        if severity == Severity::Error {
            from_findings + self.problems.len()
        } else {
            from_findings
        }
    }

    pub fn has_errors(&self) -> bool {
        self.count(Severity::Error) > 0
    }

    pub fn has_gaps(&self) -> bool {
        self.count(Severity::Gap) > 0
    }
}

/// What the check has available beyond the spec itself.
pub struct CheckContext<'a> {
    /// Known tests; `None` skips test-pointer resolution entirely.
    pub tests: Option<&'a TestSet>,
    /// Staleness oracle; `None` leaves commit-pinned reviews unassessed.
    pub oracle: Option<&'a dyn ChangeOracle>,
}

/// Consumes the loaded spec so the report owns its structural problems.
pub fn check(loaded: Loaded, ctx: &CheckContext<'_>) -> Report {
    let Loaded {
        dir,
        spec,
        problems,
    } = loaded;
    let invariants = spec
        .invariants
        .iter()
        .map(|inv| InvariantReport {
            id: inv.file.invariant.id.clone(),
            path: inv.path.clone(),
            findings: check_one(&inv.file, &dir, ctx),
        })
        .collect();
    Report {
        spec_dir: dir,
        problems,
        invariants,
        resolution: match ctx.tests {
            Some(tests) => Resolution::Resolved { tests: tests.len() },
            None => Resolution::Skipped,
        },
    }
}

fn check_one(
    file: &crate::spec::InvariantFile,
    spec_dir: &Path,
    ctx: &CheckContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let required: BTreeSet<EvidenceKind> = file.invariant.requires.iter().copied().collect();
    if required.is_empty() {
        findings.push(Finding::NothingRequired);
    }

    let Some(evidence) = &file.evidence else {
        if !required.is_empty() {
            findings.push(Finding::NoEvidence {
                required: file.invariant.requires.clone(),
            });
        }
        if let Some(review) = &file.review {
            for kind in review.0.keys() {
                findings.push(Finding::ReviewWithoutEvidence(*kind));
            }
        }
        return findings;
    };

    for kind in &required {
        if !evidence.0.contains_key(kind) {
            findings.push(Finding::MissingEvidence(*kind));
        }
    }
    for kind in evidence.0.keys() {
        if !required.contains(kind) {
            findings.push(Finding::ExtraEvidence(*kind));
        }
    }

    for (kind, pointers) in &evidence.0 {
        for pointer in pointers.iter() {
            if !resolves(*kind, pointer, spec_dir, ctx) {
                findings.push(Finding::UnresolvedPointer {
                    kind: *kind,
                    pointer: pointer.to_owned(),
                    scheme: kind.pointer_scheme(),
                });
            }
        }
    }

    let empty = Review::default();
    let review = file.review.as_ref().unwrap_or(&empty);
    for kind in review.0.keys() {
        if !evidence.0.contains_key(kind) {
            findings.push(Finding::ReviewWithoutEvidence(*kind));
        }
    }
    let default_entry = ReviewEntry::default();
    for (kind, pointers) in &evidence.0 {
        let entry = review.0.get(kind).unwrap_or(&default_entry);
        let pointers: Vec<&str> = pointers.iter().collect();
        for (by, mark) in [
            (Reviewer::Agent, &entry.agent),
            (Reviewer::Human, &entry.human),
        ] {
            match mark.state() {
                ReviewState::Unreviewed => findings.push(Finding::Unreviewed { kind: *kind, by }),
                ReviewState::Reviewed => {}
                ReviewState::ReviewedAt(commit) => {
                    if let Some(finding) =
                        assess_staleness(*kind, by, commit, &pointers, spec_dir, ctx)
                    {
                        findings.push(finding);
                    }
                }
            }
        }
    }
    findings
}

/// Whether a pointer names something that exists, per its kind's scheme.
/// Test pointers are considered resolved when no test index is available.
fn resolves(kind: EvidenceKind, pointer: &str, spec_dir: &Path, ctx: &CheckContext<'_>) -> bool {
    match kind.pointer_scheme() {
        PointerScheme::Test => ctx.tests.is_none_or(|tests| tests.contains(pointer)),
        PointerScheme::Provider => LINT_PROVIDERS.contains(&pointer),
        PointerScheme::File => spec_dir.join(pointer).is_file(),
    }
}

/// `None` means the pinned review still stands.
fn assess_staleness(
    kind: EvidenceKind,
    by: Reviewer,
    commit: &str,
    pointers: &[&str],
    spec_dir: &Path,
    ctx: &CheckContext<'_>,
) -> Option<Finding> {
    let unknown = || {
        Some(Finding::StalenessUnknown {
            kind,
            by,
            commit: commit.to_owned(),
        })
    };
    let Some(oracle) = ctx.oracle else {
        return unknown();
    };
    match oracle.commit_exists(commit) {
        Ok(true) => {}
        Ok(false) => {
            return Some(Finding::UnknownCommit {
                kind,
                by,
                commit: commit.to_owned(),
            });
        }
        Err(error) => {
            return Some(Finding::OracleFailed {
                message: error.to_string(),
            });
        }
    }
    let paths = reviewed_paths(kind, pointers, spec_dir, ctx);
    if paths.is_empty() {
        return unknown();
    }
    match oracle.changed_since(commit, &paths) {
        Ok(true) => Some(Finding::StaleReview {
            kind,
            by,
            commit: commit.to_owned(),
        }),
        Ok(false) => None,
        Err(error) => Some(Finding::OracleFailed {
            message: error.to_string(),
        }),
    }
}

/// The sources a review of this evidence covers.
fn reviewed_paths(
    kind: EvidenceKind,
    pointers: &[&str],
    spec_dir: &Path,
    ctx: &CheckContext<'_>,
) -> Vec<PathBuf> {
    match kind.pointer_scheme() {
        PointerScheme::Test => {
            let Some(tests) = ctx.tests else {
                return Vec::new();
            };
            let mut paths: Vec<PathBuf> = pointers
                .iter()
                .filter_map(|p| pointer_target(p))
                .filter_map(|target| tests.target_paths(target))
                .flat_map(|paths| paths.iter().cloned())
                .collect();
            paths.sort();
            paths.dedup();
            paths
        }
        PointerScheme::File => pointers.iter().map(|p| spec_dir.join(p)).collect(),
        PointerScheme::Provider => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitError;
    use crate::spec::InvariantFile;
    use crate::spec::load::{LoadedInvariant, Spec};

    fn loaded(text: &str) -> Loaded {
        Loaded {
            dir: PathBuf::from("/spec"),
            spec: Spec {
                invariants: vec![LoadedInvariant {
                    path: PathBuf::from("/spec/x.toml"),
                    file: InvariantFile::parse(text).expect("fixture parses"),
                }],
            },
            problems: Vec::new(),
        }
    }

    fn findings(text: &str, ctx: &CheckContext<'_>) -> Vec<Finding> {
        let report = check(loaded(text), ctx);
        report
            .invariants
            .into_iter()
            .next()
            .expect("one invariant")
            .findings
    }

    const HEAD: &str = "[invariant]\nid = \"x\"\nstatement = \"s\"\nkind = \"domain\"\n";

    fn no_ctx() -> CheckContext<'static> {
        CheckContext {
            tests: None,
            oracle: None,
        }
    }

    #[test]
    fn nothing_required_is_info_only() {
        let found = findings(HEAD, &no_ctx());
        assert_eq!(found, [Finding::NothingRequired]);
    }

    #[test]
    fn no_evidence_section_lists_everything_required() {
        let text = format!("{HEAD}requires = [\"property\", \"dst\"]\n");
        let found = findings(&text, &no_ctx());
        assert_eq!(
            found,
            [Finding::NoEvidence {
                required: vec![EvidenceKind::Property, EvidenceKind::Dst]
            }]
        );
        assert_eq!(found[0].severity(), Severity::Gap);
    }

    #[test]
    fn partial_evidence_reports_each_missing_kind_and_extras() {
        let text = format!(
            "{HEAD}requires = [\"property\", \"dst\"]\n[evidence]\nproperty = \"t::a\"\nlint = \"triglint\"\n"
        );
        let found = findings(&text, &no_ctx());
        assert!(found.contains(&Finding::MissingEvidence(EvidenceKind::Dst)));
        assert!(found.contains(&Finding::ExtraEvidence(EvidenceKind::Lint)));
        assert!(
            !found
                .iter()
                .any(|f| matches!(f, Finding::NoEvidence { .. }))
        );
    }

    #[test]
    fn test_pointers_resolve_against_the_index_when_present() {
        let text = format!(
            "{HEAD}requires = [\"example\"]\n[evidence]\nexample = [\"t::a\", \"t::missing\"]\n[review]\nexample = {{ agent = true, human = true }}\n"
        );
        let tests = TestSet::from_names(["t::a"]);
        let ctx = CheckContext {
            tests: Some(&tests),
            oracle: None,
        };
        let found = findings(&text, &ctx);
        assert_eq!(
            found,
            [Finding::UnresolvedPointer {
                kind: EvidenceKind::Example,
                pointer: "t::missing".to_owned(),
                scheme: PointerScheme::Test,
            }]
        );
        // Without an index, test pointers are taken on faith.
        assert!(findings(&text, &no_ctx()).is_empty());
    }

    #[test]
    fn lint_pointers_must_name_a_known_provider() {
        let text = format!(
            "{HEAD}requires = [\"lint\"]\n[evidence]\nlint = [\"triglint\", \"mystery\"]\n[review]\nlint = {{ agent = true, human = true }}\n"
        );
        let found = findings(&text, &no_ctx());
        assert_eq!(
            found,
            [Finding::UnresolvedPointer {
                kind: EvidenceKind::Lint,
                pointer: "mystery".to_owned(),
                scheme: PointerScheme::Provider,
            }]
        );
    }

    #[test]
    fn evidence_without_review_is_unreviewed_by_both() {
        let text = format!("{HEAD}requires = [\"lint\"]\n[evidence]\nlint = \"triglint\"\n");
        let found = findings(&text, &no_ctx());
        assert_eq!(
            found,
            [
                Finding::Unreviewed {
                    kind: EvidenceKind::Lint,
                    by: Reviewer::Agent
                },
                Finding::Unreviewed {
                    kind: EvidenceKind::Lint,
                    by: Reviewer::Human
                },
            ]
        );
    }

    #[test]
    fn reviewing_undeclared_evidence_is_an_error() {
        let text = format!(
            "{HEAD}requires = [\"lint\"]\n[evidence]\nlint = \"triglint\"\n[review]\nlint = {{ agent = true, human = true }}\ndst = {{ agent = true }}\n"
        );
        let found = findings(&text, &no_ctx());
        assert_eq!(found, [Finding::ReviewWithoutEvidence(EvidenceKind::Dst)]);
        assert_eq!(found[0].severity(), Severity::Error);
        // Also without any evidence section at all.
        let text = format!("{HEAD}requires = [\"lint\"]\n[review]\nlint = {{ agent = true }}\n");
        let found = findings(&text, &no_ctx());
        assert!(found.contains(&Finding::ReviewWithoutEvidence(EvidenceKind::Lint)));
    }

    struct FakeOracle {
        exists: bool,
        changed: bool,
    }

    impl ChangeOracle for FakeOracle {
        fn commit_exists(&self, _commit: &str) -> Result<bool, GitError> {
            Ok(self.exists)
        }

        fn changed_since(&self, _commit: &str, paths: &[PathBuf]) -> Result<bool, GitError> {
            assert!(!paths.is_empty(), "never asked without paths");
            Ok(self.changed)
        }
    }

    fn pinned() -> String {
        format!(
            "{HEAD}requires = [\"example\"]\n[evidence]\nexample = \"t::a\"\n[review]\nexample = {{ agent = true, human = \"abc123\" }}\n"
        )
    }

    fn indexed_tests() -> TestSet {
        let mut tests = TestSet::from_names(["t::a"]);
        tests.insert_target("t", vec![PathBuf::from("/w/t/src")]);
        tests
    }

    #[test]
    fn pinned_review_is_fine_when_sources_are_unchanged() {
        let tests = indexed_tests();
        let oracle = FakeOracle {
            exists: true,
            changed: false,
        };
        let ctx = CheckContext {
            tests: Some(&tests),
            oracle: Some(&oracle),
        };
        assert!(findings(&pinned(), &ctx).is_empty());
    }

    #[test]
    fn pinned_review_goes_stale_when_sources_change() {
        let tests = indexed_tests();
        let oracle = FakeOracle {
            exists: true,
            changed: true,
        };
        let ctx = CheckContext {
            tests: Some(&tests),
            oracle: Some(&oracle),
        };
        assert_eq!(
            findings(&pinned(), &ctx),
            [Finding::StaleReview {
                kind: EvidenceKind::Example,
                by: Reviewer::Human,
                commit: "abc123".to_owned(),
            }]
        );
    }

    #[test]
    fn pinned_review_on_unknown_commit_is_an_error() {
        let tests = indexed_tests();
        let oracle = FakeOracle {
            exists: false,
            changed: false,
        };
        let ctx = CheckContext {
            tests: Some(&tests),
            oracle: Some(&oracle),
        };
        let found = findings(&pinned(), &ctx);
        assert!(matches!(found.as_slice(), [Finding::UnknownCommit { .. }]));
        assert_eq!(found[0].severity(), Severity::Error);
    }

    #[test]
    fn pinned_review_without_oracle_or_paths_is_unknown_not_wrong() {
        let found = findings(&pinned(), &no_ctx());
        assert!(matches!(
            found.as_slice(),
            [Finding::StalenessUnknown { .. }]
        ));
        assert_eq!(found[0].severity(), Severity::Info);
        // Oracle present but the test's target has no known sources.
        let tests = TestSet::from_names(["t::a"]);
        let oracle = FakeOracle {
            exists: true,
            changed: true,
        };
        let ctx = CheckContext {
            tests: Some(&tests),
            oracle: Some(&oracle),
        };
        assert!(matches!(
            findings(&pinned(), &ctx).as_slice(),
            [Finding::StalenessUnknown { .. }]
        ));
    }

    #[test]
    fn report_counts_problems_as_errors() {
        let mut loaded = loaded(HEAD);
        loaded.problems.push(SpecProblem::FilenameMismatch {
            path: PathBuf::from("/spec/x.toml"),
            id: "y".to_owned(),
        });
        let report = check(loaded, &no_ctx());
        assert_eq!(report.count(Severity::Error), 1);
        assert!(report.has_errors());
        assert!(!report.has_gaps());
    }
}
