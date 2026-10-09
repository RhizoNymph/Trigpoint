//! The stopgap spec format: one invariant per file.
//!
//! A file holds exactly one `[invariant]` table, an optional `[evidence]`
//! table keyed by evidence kind, and an optional `[review]` table keyed the
//! same way. `requires` is the contract: it lists the evidence kinds the
//! invariant owes, so the check can report what is missing even before any
//! `[evidence]` exists.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

pub mod diff;
pub mod load;

/// The kinds of evidence an invariant can require or point at. The vocabulary
/// is deliberately closed: a kind outside it is a parse error, so a typo in
/// `requires` can never silently mean "nothing required".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceKind {
    /// A plain example-based test.
    Example,
    /// A property-based test (proptest, hypothesis, ...).
    Property,
    /// A deterministic-simulation scenario.
    Dst,
    /// A static-analysis evidence statement from a lint provider.
    Lint,
    /// A model-checking artifact (TLA+/Quint spec and run).
    Model,
    /// A machine-checked proof artifact.
    Proof,
}

impl EvidenceKind {
    pub const ALL: [EvidenceKind; 6] = [
        EvidenceKind::Example,
        EvidenceKind::Property,
        EvidenceKind::Dst,
        EvidenceKind::Lint,
        EvidenceKind::Model,
        EvidenceKind::Proof,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceKind::Example => "example",
            EvidenceKind::Property => "property",
            EvidenceKind::Dst => "dst",
            EvidenceKind::Lint => "lint",
            EvidenceKind::Model => "model",
            EvidenceKind::Proof => "proof",
        }
    }

    /// How a pointer of this kind is resolved.
    pub fn pointer_scheme(self) -> PointerScheme {
        match self {
            EvidenceKind::Example | EvidenceKind::Property | EvidenceKind::Dst => {
                PointerScheme::Test
            }
            EvidenceKind::Lint => PointerScheme::Provider,
            EvidenceKind::Model | EvidenceKind::Proof => PointerScheme::File,
        }
    }
}

impl fmt::Display for EvidenceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An evidence kind name that is not in the vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKind(pub String);

impl fmt::Display for UnknownKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown evidence kind `{}` (expected one of: ", self.0)?;
        for (i, kind) in EvidenceKind::ALL.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(kind.as_str())?;
        }
        f.write_str(")")
    }
}

impl std::error::Error for UnknownKind {}

impl FromStr for EvidenceKind {
    type Err = UnknownKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EvidenceKind::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| UnknownKind(s.to_owned()))
    }
}

/// What a pointer string denotes, by evidence kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerScheme {
    /// `<target>::<path::to::test>`, exactly as the test runner lists it.
    Test,
    /// The name of a lint provider (`triglint`, `trigpoint-pylint`).
    Provider,
    /// A path, relative to the spec directory, that must exist.
    File,
}

impl PointerScheme {
    pub fn describe(self) -> &'static str {
        match self {
            PointerScheme::Test => "test",
            PointerScheme::Provider => "lint provider",
            PointerScheme::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvariantKind {
    /// Derived from a capability decision (consistency model, time, ...).
    System,
    /// Specific to what the system stores or computes.
    Domain,
}

/// Hughes' classification of a property, for domain invariants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PropertyClass {
    Validity,
    Postcondition,
    Metamorphic,
    Inductive,
    ModelBased,
}

/// The `[invariant]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invariant {
    /// Stable dotted name, e.g. `kv.put.last-writer-wins`. Must equal the
    /// file stem.
    pub id: String,
    /// The claim, as one sentence.
    pub statement: String,
    pub kind: InvariantKind,
    #[serde(default)]
    pub property: Option<PropertyClass>,
    /// Evidence kinds this invariant owes.
    #[serde(default)]
    pub requires: Vec<EvidenceKind>,
    #[serde(default)]
    pub rationale: Option<String>,
    /// The decision that implied this invariant, for system invariants.
    /// Free-form until decisions are first-class.
    #[serde(default)]
    pub derived_from: Option<String>,
}

/// One pointer or several, per evidence kind.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Pointers {
    One(String),
    Many(Vec<String>),
}

impl Pointers {
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        let slice: &[String] = match self {
            Pointers::One(one) => std::slice::from_ref(one),
            Pointers::Many(many) => many.as_slice(),
        };
        slice.iter().map(String::as_str)
    }
}

/// The `[evidence]` table: evidence kind → pointer(s).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "BTreeMap<String, Pointers>")]
pub struct Evidence(pub BTreeMap<EvidenceKind, Pointers>);

impl TryFrom<BTreeMap<String, Pointers>> for Evidence {
    type Error = UnknownKind;

    fn try_from(raw: BTreeMap<String, Pointers>) -> Result<Self, Self::Error> {
        kind_keyed(raw).map(Evidence)
    }
}

/// Whether, and as of what, a reviewer has assessed a piece of evidence.
/// `true`/`false` are the hand-written form; a commit hash pins the review
/// to a revision so the check can tell when it has gone stale.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ReviewMark {
    Flag(bool),
    Commit(String),
}

impl Default for ReviewMark {
    fn default() -> Self {
        ReviewMark::Flag(false)
    }
}

impl ReviewMark {
    pub fn state(&self) -> ReviewState<'_> {
        match self {
            ReviewMark::Flag(false) => ReviewState::Unreviewed,
            ReviewMark::Flag(true) => ReviewState::Reviewed,
            ReviewMark::Commit(commit) => ReviewState::ReviewedAt(commit),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState<'a> {
    Unreviewed,
    Reviewed,
    ReviewedAt(&'a str),
}

/// One `[review]` entry: who has assessed this kind of evidence.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewEntry {
    #[serde(default)]
    pub agent: ReviewMark,
    #[serde(default)]
    pub human: ReviewMark,
}

/// The `[review]` table: evidence kind → review entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "BTreeMap<String, ReviewEntry>")]
pub struct Review(pub BTreeMap<EvidenceKind, ReviewEntry>);

impl TryFrom<BTreeMap<String, ReviewEntry>> for Review {
    type Error = UnknownKind;

    fn try_from(raw: BTreeMap<String, ReviewEntry>) -> Result<Self, Self::Error> {
        kind_keyed(raw).map(Review)
    }
}

fn kind_keyed<T>(raw: BTreeMap<String, T>) -> Result<BTreeMap<EvidenceKind, T>, UnknownKind> {
    raw.into_iter()
        .map(|(key, value)| key.parse::<EvidenceKind>().map(|kind| (kind, value)))
        .collect()
}

/// One `spec/invariants/<id>.toml` file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvariantFile {
    pub invariant: Invariant,
    #[serde(default)]
    pub evidence: Option<Evidence>,
    #[serde(default)]
    pub review: Option<Review>,
}

impl InvariantFile {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
        [invariant]
        id = "kv.put.last-writer-wins"
        statement = """
        Two puts to the same key leave the later one observable by get.
        """
        kind = "domain"
        property = "metamorphic"
        requires = ["property", "dst"]
        rationale = "Hot keys race."

        [evidence]
        property = "kv::tests::put_then_get_returns_latest"
        dst = ["kv::tests::torn_write", "kv::tests::partition"]

        [review]
        property = { agent = true, human = "9b84f15" }
        dst = { agent = false }
    "#;

    #[test]
    fn full_file_parses() {
        let file = InvariantFile::parse(FULL).expect("should parse");
        assert_eq!(file.invariant.id, "kv.put.last-writer-wins");
        assert_eq!(file.invariant.kind, InvariantKind::Domain);
        assert_eq!(file.invariant.property, Some(PropertyClass::Metamorphic));
        assert_eq!(
            file.invariant.requires,
            [EvidenceKind::Property, EvidenceKind::Dst]
        );
        let evidence = file.evidence.expect("evidence present");
        let property: Vec<&str> = evidence.0[&EvidenceKind::Property].iter().collect();
        assert_eq!(property, ["kv::tests::put_then_get_returns_latest"]);
        let dst: Vec<&str> = evidence.0[&EvidenceKind::Dst].iter().collect();
        assert_eq!(dst, ["kv::tests::torn_write", "kv::tests::partition"]);
        let review = file.review.expect("review present");
        let property = &review.0[&EvidenceKind::Property];
        assert_eq!(property.agent.state(), ReviewState::Reviewed);
        assert_eq!(property.human.state(), ReviewState::ReviewedAt("9b84f15"));
        let dst = &review.0[&EvidenceKind::Dst];
        assert_eq!(dst.agent.state(), ReviewState::Unreviewed);
        assert_eq!(dst.human.state(), ReviewState::Unreviewed);
    }

    #[test]
    fn minimal_file_parses_without_evidence_or_review() {
        let file = InvariantFile::parse(
            "[invariant]\nid = \"a.b\"\nstatement = \"s\"\nkind = \"system\"\n",
        )
        .expect("should parse");
        assert!(file.invariant.requires.is_empty());
        assert!(file.evidence.is_none());
        assert!(file.review.is_none());
    }

    #[test]
    fn unknown_required_kind_is_an_error() {
        let error = InvariantFile::parse(
            "[invariant]\nid = \"a\"\nstatement = \"s\"\nkind = \"system\"\nrequires = [\"propety\"]\n",
        )
        .expect_err("typo should fail");
        assert!(error.to_string().contains("propety"), "{error}");
    }

    #[test]
    fn unknown_evidence_key_names_the_vocabulary() {
        let error = InvariantFile::parse(
            "[invariant]\nid = \"a\"\nstatement = \"s\"\nkind = \"system\"\n[evidence]\npropety = \"x\"\n",
        )
        .expect_err("typo should fail");
        let message = error.to_string();
        assert!(
            message.contains("unknown evidence kind `propety`"),
            "{message}"
        );
        assert!(message.contains("property"), "{message}");
    }

    #[test]
    fn unknown_invariant_field_is_an_error() {
        let error = InvariantFile::parse(
            "[invariant]\nid = \"a\"\nstatement = \"s\"\nkind = \"system\"\nrequire = []\n",
        )
        .expect_err("unknown field should fail");
        assert!(error.to_string().contains("require"), "{error}");
    }

    #[test]
    fn review_mark_rejects_non_bool_non_string() {
        let error = InvariantFile::parse(
            "[invariant]\nid = \"a\"\nstatement = \"s\"\nkind = \"system\"\n[review]\nlint = { agent = 1 }\n",
        )
        .expect_err("integer mark should fail");
        assert!(!error.to_string().is_empty());
    }
}
