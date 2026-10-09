//! Resolving test pointers against the tests that actually exist.
//!
//! Pointers use the test runner's own naming — `<target>::<path::to::test>`
//! — so resolution is a set lookup against what `cargo test -- --list`
//! reports, not a parse of the sources. The target segment is the crate
//! name (underscores) for unit tests and the file stem for integration
//! tests, exactly as cargo names the test binaries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Something that knows which tests exist.
pub trait TestIndex {
    fn contains(&self, pointer: &str) -> bool;
}

/// The known tests, plus the source paths behind each test target so a
/// commit-pinned review can be checked for staleness.
#[derive(Debug, Default, Clone)]
pub struct TestSet {
    names: BTreeSet<String>,
    targets: BTreeMap<String, Vec<PathBuf>>,
}

impl TestSet {
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
            targets: BTreeMap::new(),
        }
    }

    pub fn insert_test(&mut self, name: impl Into<String>) {
        self.names.insert(name.into());
    }

    pub fn insert_target(&mut self, target: impl Into<String>, paths: Vec<PathBuf>) {
        self.targets.insert(target.into(), paths);
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Source paths whose changes could invalidate a review of a test in
    /// `target`. `None` when the target is unknown.
    pub fn target_paths(&self, target: &str) -> Option<&[PathBuf]> {
        self.targets.get(target).map(Vec::as_slice)
    }
}

impl TestIndex for TestSet {
    fn contains(&self, pointer: &str) -> bool {
        self.names.contains(pointer)
    }
}

/// The target segment of a test pointer: everything before the first `::`.
pub fn pointer_target(pointer: &str) -> Option<&str> {
    pointer.split_once("::").map(|(target, _)| target)
}

/// Collecting the test set from a cargo workspace.
pub mod cargo {
    use std::path::Path;
    use std::process::Command;

    use thiserror::Error;

    use super::TestSet;

    #[derive(Debug, Error)]
    pub enum CargoError {
        #[error("failed to run {what}: {source}")]
        Spawn {
            what: String,
            #[source]
            source: std::io::Error,
        },
        #[error("`cargo test --no-run` failed:\n{stderr}")]
        Build { stderr: String },
        #[error("could not parse cargo's JSON output: {0}")]
        Json(#[from] serde_json::Error),
        #[error("test binary {path} failed to list its tests:\n{stderr}")]
        List { path: String, stderr: String },
    }

    /// One test executable cargo built.
    struct TestBinary {
        target: String,
        executable: String,
        source_paths: Vec<std::path::PathBuf>,
    }

    /// Builds the workspace's tests and asks every test binary what it
    /// contains.
    pub fn collect(dir: &Path) -> Result<TestSet, CargoError> {
        let output = Command::new("cargo")
            .args(["test", "--workspace", "--no-run", "--message-format=json"])
            .current_dir(dir)
            .output()
            .map_err(|source| CargoError::Spawn {
                what: "cargo test --no-run".to_owned(),
                source,
            })?;
        if !output.status.success() {
            return Err(CargoError::Build {
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let binaries = parse_artifacts(&stdout)?;

        let mut set = TestSet::default();
        for binary in binaries {
            let listing = Command::new(&binary.executable)
                .arg("--list")
                .current_dir(dir)
                .output()
                .map_err(|source| CargoError::Spawn {
                    what: format!("{} --list", binary.executable),
                    source,
                })?;
            if !listing.status.success() {
                return Err(CargoError::List {
                    path: binary.executable.clone(),
                    stderr: String::from_utf8_lossy(&listing.stderr).into_owned(),
                });
            }
            for name in parse_listing(&String::from_utf8_lossy(&listing.stdout)) {
                set.insert_test(format!("{}::{name}", binary.target));
            }
            set.insert_target(binary.target, binary.source_paths);
        }
        Ok(set)
    }

    /// Picks the test executables out of cargo's JSON message stream.
    fn parse_artifacts(stdout: &str) -> Result<Vec<TestBinary>, CargoError> {
        let mut binaries = Vec::new();
        for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
            let message: serde_json::Value = serde_json::from_str(line)?;
            if message["reason"] != "compiler-artifact" || message["profile"]["test"] != true {
                continue;
            }
            let Some(executable) = message["executable"].as_str() else {
                continue;
            };
            let target = &message["target"];
            let Some(name) = target["name"].as_str() else {
                continue;
            };
            let kind = target["kind"][0].as_str().unwrap_or("lib");
            let src_path = target["src_path"]
                .as_str()
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            binaries.push(TestBinary {
                target: name.replace('-', "_"),
                executable: executable.to_owned(),
                source_paths: source_paths_for(kind, &src_path),
            });
        }
        Ok(binaries)
    }

    /// The sources a test target is built from: the crate's `src/` for
    /// unit tests, the file (and its sibling module dir) for integration
    /// tests.
    fn source_paths_for(kind: &str, src_path: &Path) -> Vec<std::path::PathBuf> {
        match kind {
            "test" => {
                let mut paths = vec![src_path.to_owned()];
                paths.push(src_path.with_extension(""));
                paths
            }
            _ => src_path
                .parent()
                .map(|parent| vec![parent.to_owned()])
                .unwrap_or_default(),
        }
    }

    /// Test names from `<binary> --list` output (`name: test` lines).
    fn parse_listing(stdout: &str) -> Vec<String> {
        stdout
            .lines()
            .filter_map(|line| line.strip_suffix(": test"))
            .map(str::to_owned)
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn listing_keeps_only_tests() {
            let names = parse_listing(
                "tests::a: test\ntests::b: test\nbench_x: bench\n\n2 tests, 1 benchmark\n",
            );
            assert_eq!(names, ["tests::a", "tests::b"]);
        }

        #[test]
        fn artifacts_pick_test_executables_and_normalize_names() {
            let stream = concat!(
                r#"{"reason":"compiler-artifact","target":{"name":"demo-lib","kind":["lib"],"src_path":"/w/demo-lib/src/lib.rs"},"profile":{"test":false},"executable":null}"#,
                "\n",
                r#"{"reason":"compiler-artifact","target":{"name":"demo-lib","kind":["lib"],"src_path":"/w/demo-lib/src/lib.rs"},"profile":{"test":true},"executable":"/w/target/debug/deps/demo_lib-abc"}"#,
                "\n",
                r#"{"reason":"compiler-artifact","target":{"name":"fixtures","kind":["test"],"src_path":"/w/demo-lib/tests/fixtures.rs"},"profile":{"test":true},"executable":"/w/target/debug/deps/fixtures-abc"}"#,
                "\n",
                r#"{"reason":"build-finished","success":true}"#,
                "\n",
            );
            let binaries = parse_artifacts(stream).expect("parses");
            assert_eq!(binaries.len(), 2);
            assert_eq!(binaries[0].target, "demo_lib");
            assert_eq!(
                binaries[0].source_paths,
                [std::path::PathBuf::from("/w/demo-lib/src")]
            );
            assert_eq!(binaries[1].target, "fixtures");
            assert_eq!(
                binaries[1].source_paths,
                [
                    std::path::PathBuf::from("/w/demo-lib/tests/fixtures.rs"),
                    std::path::PathBuf::from("/w/demo-lib/tests/fixtures"),
                ]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn set_lookup_and_target_segment() {
        let set = TestSet::from_names(["demo_lib::tests::ticks"]);
        assert!(set.contains("demo_lib::tests::ticks"));
        assert!(!set.contains("demo_lib::tests::nope"));
        assert_eq!(pointer_target("demo_lib::tests::ticks"), Some("demo_lib"));
        assert_eq!(pointer_target("no-separator"), None);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn target_paths_default_to_unknown() {
        let mut set = TestSet::default();
        assert!(set.target_paths("x").is_none());
        set.insert_target("x", vec![Path::new("/src").to_owned()]);
        assert_eq!(
            set.target_paths("x"),
            Some(&[Path::new("/src").to_owned()][..])
        );
    }
}
