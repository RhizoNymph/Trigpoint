//! Loading a spec directory: every `*.toml` file is one invariant.
//!
//! Structural problems (unreadable or unparseable files, a filename that
//! does not match its `id`, duplicate ids) are collected rather than
//! aborting, so one bad file never hides the report for the rest.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::InvariantFile;

/// One successfully parsed invariant file.
#[derive(Debug, Clone)]
pub struct LoadedInvariant {
    pub path: PathBuf,
    pub file: InvariantFile,
}

/// The parsed spec, sorted by invariant id.
#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub invariants: Vec<LoadedInvariant>,
}

/// A structural problem with the spec directory. Always an error.
#[derive(Debug, Error)]
pub enum SpecProblem {
    #[error("{path}: failed to read: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: failed to parse: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{path}: file stem does not match invariant id `{id}`")]
    FilenameMismatch { path: PathBuf, id: String },
    #[error("invariant id `{id}` is declared in more than one file: {}", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    DuplicateId { id: String, paths: Vec<PathBuf> },
}

/// Result of loading: whatever parsed, plus whatever did not.
#[derive(Debug, Default)]
pub struct Loaded {
    pub dir: PathBuf,
    pub spec: Spec,
    pub problems: Vec<SpecProblem>,
}

/// Failures that make loading impossible rather than partially successful.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("spec directory {path} does not exist")]
    MissingDir { path: PathBuf },
    #[error("failed to read spec directory {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Loads every `*.toml` file directly inside `dir` (not recursive).
pub fn load_dir(dir: &Path) -> Result<Loaded, LoadError> {
    if !dir.is_dir() {
        return Err(LoadError::MissingDir {
            path: dir.to_owned(),
        });
    }
    let entries = fs::read_dir(dir).map_err(|source| LoadError::ReadDir {
        path: dir.to_owned(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml") && path.is_file())
        .collect();
    paths.sort();

    let mut loaded = Loaded {
        dir: dir.to_owned(),
        ..Loaded::default()
    };
    let mut by_id: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in paths {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) => {
                loaded.problems.push(SpecProblem::Read { path, source });
                continue;
            }
        };
        let file = match InvariantFile::parse(&text) {
            Ok(file) => file,
            Err(source) => {
                loaded.problems.push(SpecProblem::Parse { path, source });
                continue;
            }
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem != file.invariant.id {
            loaded.problems.push(SpecProblem::FilenameMismatch {
                path: path.clone(),
                id: file.invariant.id.clone(),
            });
        }
        by_id
            .entry(file.invariant.id.clone())
            .or_default()
            .push(path.clone());
        loaded.spec.invariants.push(LoadedInvariant { path, file });
    }
    for (id, paths) in by_id {
        if paths.len() > 1 {
            loaded.problems.push(SpecProblem::DuplicateId { id, paths });
        }
    }
    loaded
        .spec
        .invariants
        .sort_by(|a, b| a.file.invariant.id.cmp(&b.file.invariant.id));
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trigpoint-core-load-{name}"));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("clean scratch dir");
        }
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).expect("write fixture");
    }

    const fn minimal(id: &str) -> [&str; 3] {
        [
            "[invariant]\nid = \"",
            id,
            "\"\nstatement = \"s\"\nkind = \"system\"\n",
        ]
    }

    fn minimal_text(id: &str) -> String {
        minimal(id).concat()
    }

    #[test]
    fn missing_dir_is_an_error() {
        let dir = std::env::temp_dir().join("trigpoint-core-load-does-not-exist");
        assert!(matches!(load_dir(&dir), Err(LoadError::MissingDir { .. })));
    }

    #[test]
    fn loads_sorted_and_reports_nothing_for_a_clean_dir() {
        let dir = scratch("clean");
        write(&dir, "b.two.toml", &minimal_text("b.two"));
        write(&dir, "a.one.toml", &minimal_text("a.one"));
        write(&dir, "notes.md", "ignored");
        let loaded = load_dir(&dir).expect("loads");
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        let ids: Vec<&str> = loaded
            .spec
            .invariants
            .iter()
            .map(|i| i.file.invariant.id.as_str())
            .collect();
        assert_eq!(ids, ["a.one", "b.two"]);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn filename_must_match_id() {
        let dir = scratch("mismatch");
        write(&dir, "wrong-name.toml", &minimal_text("a.one"));
        let loaded = load_dir(&dir).expect("loads");
        assert!(matches!(
            loaded.problems.as_slice(),
            [SpecProblem::FilenameMismatch { id, .. }] if id == "a.one"
        ));
        // The file still participates in the report.
        assert_eq!(loaded.spec.invariants.len(), 1);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn duplicate_ids_are_reported_once_with_both_paths() {
        let dir = scratch("dup");
        write(&dir, "a.one.toml", &minimal_text("a.one"));
        write(&dir, "a.one-copy.toml", &minimal_text("a.one"));
        let loaded = load_dir(&dir).expect("loads");
        let duplicates: Vec<&SpecProblem> = loaded
            .problems
            .iter()
            .filter(|p| matches!(p, SpecProblem::DuplicateId { .. }))
            .collect();
        assert_eq!(duplicates.len(), 1);
        if let SpecProblem::DuplicateId { id, paths } = duplicates[0] {
            assert_eq!(id, "a.one");
            assert_eq!(paths.len(), 2);
        }
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn parse_errors_do_not_hide_other_files() {
        let dir = scratch("parse");
        write(&dir, "a.one.toml", &minimal_text("a.one"));
        write(&dir, "broken.toml", "[invariant]\nid = \n");
        let loaded = load_dir(&dir).expect("loads");
        assert!(matches!(
            loaded.problems.as_slice(),
            [SpecProblem::Parse { .. }]
        ));
        assert_eq!(loaded.spec.invariants.len(), 1);
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
