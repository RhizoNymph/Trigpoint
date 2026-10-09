//! Semantic differences between committed invariant files. Formatting, array
//! order, and the scalar/list evidence shorthand do not affect the report.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::InvariantFile;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub file: InvariantFile,
    pub facts: BTreeSet<String>,
    pub warnings: BTreeSet<String>,
}
pub type Snapshot = BTreeMap<String, Entry>;

/// Validate a file and add its normalized field/value facts to a snapshot.
pub fn insert(snapshot: &mut Snapshot, path: &Path, text: &str) -> Result<(), String> {
    let file = InvariantFile::parse(text).map_err(|e| format!("{}: {e}", path.display()))?;
    let id = file.invariant.id.clone();
    if path.file_stem().and_then(|s| s.to_str()) != Some(&id) {
        return Err(format!(
            "{}: file stem does not match invariant id `{id}`",
            path.display()
        ));
    }
    if snapshot.contains_key(&id) {
        return Err(format!("duplicate invariant id `{id}`"));
    }
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut facts = BTreeSet::new();
    flatten("", &value, &mut facts);
    facts.insert(format!("filename = {}", path.display()));
    snapshot.insert(
        id,
        Entry {
            path: path.to_owned(),
            file,
            facts,
            warnings: BTreeSet::new(),
        },
    );
    Ok(())
}

fn flatten(key: &str, value: &toml::Value, facts: &mut BTreeSet<String>) {
    match value {
        toml::Value::Table(table) => {
            for (name, value) in table {
                let key = if key.is_empty() {
                    name.clone()
                } else {
                    format!("{key}.{name}")
                };
                flatten(&key, value, facts);
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                flatten(key, value, facts);
            }
        }
        // These are implicit in the schema or already shown in the ID column.
        toml::Value::Boolean(false) if key.starts_with("review.") => {}
        _ => {
            let key = if let Some(rest) = key.strip_prefix("review.") {
                if let Some((kind, role)) = rest.split_once('.') {
                    format!("{role} review.{kind}")
                } else {
                    key.into()
                }
            } else {
                key.into()
            };
            facts.insert(format!("{key} = {value}"));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub id: String,
    pub previous_id: Option<String>,
    pub created: bool,
    pub removed: bool,
    pub invalid_kind: bool,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    pub reviews: BTreeSet<String>,
    pub warnings: BTreeSet<String>,
}

pub fn compare(base: &Snapshot, head: &Snapshot) -> Vec<Change> {
    compare_with_renames(base, head, &BTreeMap::new())
}

/// Rename pairs are old/new repository-relative filenames from Git. If Git
/// misses an ID-only rename, pair uniquely identical declarations ourselves.
pub fn compare_with_renames(
    base: &Snapshot,
    head: &Snapshot,
    renames: &BTreeMap<PathBuf, PathBuf>,
) -> Vec<Change> {
    let mut pairs = BTreeMap::new();
    for (id, old) in base {
        if head.contains_key(id) {
            pairs.insert(id.clone(), id.clone());
            continue;
        }
        let candidates: Vec<_> = head
            .iter()
            .filter(|(new_id, new)| {
                !base.contains_key(*new_id) && renames.get(&old.path) == Some(&new.path)
            })
            .collect();
        if let [(new_id, _)] = candidates.as_slice() {
            pairs.insert(id.clone(), (*new_id).clone());
        }
    }
    let signature = |entry: &Entry| {
        entry
            .facts
            .iter()
            .filter(|f| {
                !f.starts_with("invariant.id = ")
                    && !f.starts_with("filename = ")
                    && !f.starts_with("agent review.")
                    && !f.starts_with("human review.")
            })
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    for (id, old) in base {
        if pairs.contains_key(id) {
            continue;
        }
        let candidates: Vec<_> = head
            .iter()
            .filter(|(new_id, new)| {
                !base.contains_key(*new_id)
                    && !pairs.values().any(|v| v == *new_id)
                    && signature(old) == signature(new)
            })
            .collect();
        if let [(new_id, new)] = candidates.as_slice() {
            let reverse = base
                .iter()
                .filter(|(old_id, entry)| {
                    !head.contains_key(*old_id) && signature(entry) == signature(new)
                })
                .count();
            if reverse == 1 {
                pairs.insert(id.clone(), (*new_id).clone());
            }
        }
    }
    let mut result = Vec::new();
    for (id, old) in base {
        let new = pairs.get(id).and_then(|id| head.get(id));
        if let Some(change) = change(Some(old), new) {
            result.push(change);
        }
    }
    for (id, new) in head {
        if !pairs.values().any(|v| v == id) {
            if let Some(change) = change(None, Some(new)) {
                result.push(change);
            }
        }
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    result
}

fn change(old: Option<&Entry>, new: Option<&Entry>) -> Option<Change> {
    let empty = BTreeSet::new();
    let before = old.map(|e| &e.facts).unwrap_or(&empty);
    let after = new.map(|e| &e.facts).unwrap_or(&empty);
    let added: Vec<_> = after.difference(before).cloned().collect();
    let deleted: Vec<_> = before.difference(after).cloned().collect();
    let selected = new.or(old).expect("one side exists");
    let warnings = old
        .into_iter()
        .chain(new)
        .flat_map(|e| e.warnings.iter().cloned())
        .collect::<BTreeSet<_>>();
    if old.is_some() && new.is_some() && added.is_empty() && deleted.is_empty() {
        return None;
    }
    Some(Change {
        id: selected.file.invariant.id.clone(),
        previous_id: old
            .zip(new)
            .filter(|(a, b)| a.file.invariant.id != b.file.invariant.id)
            .map(|(a, _)| a.file.invariant.id.clone()),
        created: old.is_none(),
        removed: new.is_none(),
        invalid_kind: old
            .zip(new)
            .is_some_and(|(a, b)| a.file.invariant.kind != b.file.invariant.kind),
        added,
        deleted,
        reviews: selected
            .facts
            .iter()
            .filter(|f| f.starts_with("agent review.") || f.starts_with("human review."))
            .cloned()
            .collect(),
        warnings,
    })
}

/// Escape untrusted spec content for a GitHub Markdown table.
pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '|' => "&#124;".to_owned(),
            '\n' => "<br>".to_owned(),
            '\r' => String::new(),
            '`' | '*' | '_' | '[' | ']' | '\\' | '~' | '@' => format!("&#{};", c as u32),
            _ => c.to_string(),
        })
        .collect()
}

const COLUMNS: &[(&str, &str)] = &[
    ("ID", "invariant.id"),
    ("Filename", "filename"),
    ("Statement", "invariant.statement"),
    ("Kind (immutable)", "invariant.kind"),
    ("Property", "invariant.property"),
    ("Requires", "invariant.requires"),
    ("Rationale", "invariant.rationale"),
    ("Derived from", "invariant.derived_from"),
    ("Evidence", "evidence"),
    ("Test body", "test body"),
    ("Test docstring", "test docstring"),
    ("Agent review", "agent review"),
    ("Human review", "human review"),
];

fn belongs(fact: &str, field: &str) -> bool {
    fact.strip_prefix(field)
        .is_some_and(|rest| rest.starts_with(" = ") || rest.starts_with('.'))
}

/// A compact line diff: preserve the changed interval, omit common prefix and
/// suffix lines. Every changed value is shown; no HTML from the spec is trusted.
fn cell(change: &Change, field: &str) -> String {
    let values = |facts: &[String]| {
        facts
            .iter()
            .filter(|f| belongs(f, field))
            .map(|f| {
                f.strip_prefix(&format!("{field} = "))
                    .unwrap_or(f)
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    let before = values(&change.deleted);
    let after = values(&change.added);
    let mut lines = Vec::new();
    if field == "test body" && before.len() == 1 && after.len() == 1 {
        if let (Some((old, _)), Some((new, _))) =
            (before[0].split_once(" = "), after[0].split_once(" = "))
        {
            if old == new {
                lines.push(escape(old));
            }
        }
    }
    if before.len() == 1 && after.len() == 1 {
        let a: Vec<_> = before[0].lines().collect();
        let b: Vec<_> = after[0].lines().collect();
        let prefix = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
        let suffix = a[prefix..]
            .iter()
            .rev()
            .zip(b[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        lines.extend(
            a[prefix..a.len() - suffix]
                .iter()
                .map(|s| format!("− {}", escape(s))),
        );
        lines.extend(
            b[prefix..b.len() - suffix]
                .iter()
                .map(|s| format!("+ {}", escape(s))),
        );
    } else {
        for (sign, values) in [("−", before), ("+", after)] {
            for value in values {
                lines.push(format!("{sign} {}", escape(&value)));
            }
        }
    }
    if field.ends_with("review") {
        lines.extend(
            change
                .reviews
                .iter()
                .filter(|f| {
                    belongs(f, field) && !change.added.contains(f) && !change.deleted.contains(f)
                })
                .map(|f| escape(f)),
        );
    }
    if change.invalid_kind && field == "invariant.kind" {
        lines.push("**ERROR: kind cannot change**".into());
    }
    if lines.is_empty() {
        "—".into()
    } else {
        lines.join("<br>")
    }
}

pub fn render(changes: &[Change]) -> String {
    let mut out = String::from("| Invariant | Change |");
    for (name, _) in COLUMNS {
        out.push_str(&format!(" {name} |"));
    }
    out.push_str("\n| --- | --- |");
    for _ in COLUMNS {
        out.push_str(" --- |");
    }
    out.push('\n');
    for change in changes {
        let status = if change.created {
            "Created"
        } else if change.removed {
            "Removed"
        } else if change.previous_id.is_some() {
            "Renamed"
        } else {
            "Modified"
        };
        out.push_str(&format!("| {} | {status} |", escape(&change.id)));
        for (_, field) in COLUMNS {
            out.push_str(&format!(" {} |", cell(change, field)));
        }
        out.push('\n');
    }
    if changes.is_empty() {
        out.push_str("\nNo invariant changes.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(id: &str, extra: &str) -> Snapshot {
        let mut result = Snapshot::new();
        insert(
            &mut result,
            Path::new(&format!("{id}.toml")),
            &format!("[invariant]\nid = {id:?}\nstatement = 'claim'\nkind = 'system'\n{extra}"),
        )
        .unwrap();
        result
    }
    #[test]
    fn all_categories_and_replacements() {
        let mut base = snapshot("removed", "");
        base.extend(snapshot(
            "changed",
            "requires = ['example']\n[evidence]\nexample = 'old'",
        ));
        let mut head = snapshot("created", "rationale = 'new'");
        head.extend(snapshot(
            "changed",
            "requires = ['example', 'dst']\n[evidence]\nexample = 'new'",
        ));
        let changes = compare(&base, &head);
        assert_eq!(
            changes.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["changed", "created", "removed"]
        );
        assert_eq!(changes[0].added.len(), 2);
        assert_eq!(changes[0].deleted.len(), 1);
        assert!(changes[1].created);
        assert!(changes[2].removed);
    }
    #[test]
    fn equivalent_formatting_and_defaults_are_ignored() {
        let a = snapshot(
            "a",
            "requires = ['dst', 'example']\n[evidence]\nexample = 'test'\n",
        );
        let b = snapshot(
            "a",
            "requires = ['example', 'dst'] # comment\n[evidence]\nexample = ['test']\n[review]\nexample = {human = false}\n",
        );
        assert!(compare(&a, &b).is_empty());
    }
    #[test]
    fn invalid_specs_and_duplicate_ids_fail() {
        let mut s = snapshot("a", "");
        assert!(insert(&mut s, Path::new("b.toml"), "invalid").is_err());
        let valid = "[invariant]\nid='a'\nstatement='s'\nkind='system'";
        assert!(insert(&mut s, Path::new("wrong.toml"), valid).is_err());
        assert!(insert(&mut s, Path::new("a.toml"), valid).is_err());
    }
    #[test]
    fn field_columns_and_kind_immutability() {
        let base = snapshot("a", "rationale = 'old'\nrequires=['example']");
        let mut head = Snapshot::new();
        insert(&mut head, Path::new("a.toml"), "[invariant]\nid='a'\nstatement='claim'\nkind='domain'\nrationale='new'\nrequires=['example', 'property']").unwrap();
        let changes = compare(&base, &head);
        assert!(changes[0].invalid_kind);
        let report = render(&changes);
        assert!(
            report.contains("| Statement | Kind (immutable) | Property | Requires | Rationale |")
        );
        assert!(report.contains("ERROR: kind cannot change"));
        assert!(report.contains("− \"old\"<br>+ \"new\""));
        assert!(!report.contains("| Created | Removed |"));
    }
    #[test]
    fn id_only_rename_fallback_requires_a_unique_match() {
        let base = snapshot("old", "");
        let head = snapshot("new", "");
        let changes = compare(&base, &head);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].previous_id.as_deref(), Some("old"));
        let mut ambiguous = head;
        ambiguous.extend(snapshot("another", ""));
        let changes = compare(&base, &ambiguous);
        assert_eq!(changes.len(), 3);
        assert!(changes.iter().all(|c| c.previous_id.is_none()));
    }
    #[test]
    fn markdown_content_is_escaped() {
        assert_eq!(
            escape("a|<b>&`\n@all"),
            "a&#124;&lt;b&gt;&amp;&#96;<br>&#64;all"
        );
        assert!(render(&[]).contains("No invariant changes."));
    }
}
