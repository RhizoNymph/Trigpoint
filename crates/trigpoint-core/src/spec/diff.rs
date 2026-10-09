//! Semantic differences between committed invariant files. Formatting, array
//! order, and the scalar/list evidence shorthand do not affect the report.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::InvariantFile;

pub type Snapshot = BTreeMap<String, BTreeSet<String>>;

/// Validate a file and add its normalized field/value facts to a snapshot.
pub fn insert(snapshot: &mut Snapshot, path: &Path, text: &str) -> Result<(), String> {
    let file = InvariantFile::parse(text).map_err(|e| format!("{}: {e}", path.display()))?;
    let id = file.invariant.id;
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
    snapshot.insert(id, facts);
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
        _ if key == "invariant.id" => {}
        _ => {
            facts.insert(format!("{key} = {value}"));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub id: String,
    pub created: bool,
    pub removed: bool,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
}

pub fn compare(base: &Snapshot, head: &Snapshot) -> Vec<Change> {
    let ids: BTreeSet<_> = base.keys().chain(head.keys()).collect();
    let empty = BTreeSet::new();
    ids.into_iter()
        .filter_map(|id| {
            let before = base.get(id).unwrap_or(&empty);
            let after = head.get(id).unwrap_or(&empty);
            let created = !base.contains_key(id);
            let removed = !head.contains_key(id);
            let added: Vec<_> = after.difference(before).cloned().collect();
            let deleted: Vec<_> = before.difference(after).cloned().collect();
            (created || removed || !added.is_empty() || !deleted.is_empty()).then(|| Change {
                id: id.clone(),
                created,
                removed,
                added,
                deleted,
            })
        })
        .collect()
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

pub fn render(changes: &[Change]) -> String {
    let mut out = String::from(
        "| Invariant | Created | Removed | Added to | Deleted from |\n| --- | --- | --- | --- | --- |\n",
    );
    for change in changes {
        let list = |values: &[String]| {
            if values.is_empty() {
                "—".to_owned()
            } else {
                values
                    .iter()
                    .map(|v| escape(v))
                    .collect::<Vec<_>>()
                    .join("<br>")
            }
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            escape(&change.id),
            if change.created { "✓" } else { "—" },
            if change.removed { "✓" } else { "—" },
            list(&change.added),
            list(&change.deleted)
        ));
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
        let mut head = snapshot("created", "");
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
    fn markdown_content_is_escaped() {
        assert_eq!(
            escape("a|<b>&`\n@all"),
            "a&#124;&lt;b&gt;&amp;&#96;<br>&#64;all"
        );
        assert!(render(&[]).contains("No invariant changes."));
    }
}
