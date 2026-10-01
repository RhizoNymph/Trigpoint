//! The `[python]` section of `triglint.toml`: resolution and discovery.
//!
//! The schema itself lives in `trigpoint-config` (`trigpoint_config::python`),
//! the shared stable crate holding the whole `triglint.toml` contract; this
//! module re-exports it and adds what only the Python analysis needs: the
//! queryable [`Resolved`] view (which folds in the builtin sink database from
//! [`crate::sinks`]) and discovery anchored at a start directory. Parsing goes
//! through the full shared [`trigpoint_config::Config`], so the file is
//! validated strictly end to end — a typo in either the Rust or the Python
//! half is an error for every consumer.
//!
//! Discovery mirrors triglint exactly: `$TRIGLINT_CONFIG` if set, otherwise the
//! nearest `triglint.toml` walking up from a start directory.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

pub use trigpoint_config::python::{
    Markers, Opaque, PythonConfig, ShimSpec, SimSection, SinkSpec,
};
pub use trigpoint_config::{CONFIG_ENV, CONFIG_FILE};

use crate::sinks::{SinkDb, SinkRule, builtin_sinks, builtin_trusted_modules};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

/// A resolved, queryable view of `[python]`, anchored at the config file's
/// directory so `source_roots` can be made absolute exactly once.
#[derive(Debug)]
pub struct Resolved {
    root_dir: PathBuf,
    source_roots: Vec<PathBuf>,
    sim_roots: Vec<String>,
    shims: Vec<ShimSpec>,
    deterministic_markers: Vec<String>,
    sinks: SinkDb,
    trusted_modules: BTreeSet<String>,
    opaque_allow: Vec<String>,
}

impl Resolved {
    /// `root_dir` is the directory containing the config file; relative
    /// `source_roots` are resolved against it.
    pub fn new(root_dir: &Path, config: PythonConfig) -> Self {
        let mut rules: Vec<SinkRule> = if config.builtin_sinks {
            builtin_sinks()
        } else {
            Vec::new()
        };
        rules.extend(config.sinks.iter().map(SinkRule::from_spec));
        let mut trusted: BTreeSet<String> = builtin_trusted_modules()
            .iter()
            .map(|m| (*m).to_owned())
            .collect();
        trusted.extend(config.opaque.trusted_modules.iter().cloned());
        Self {
            root_dir: root_dir.to_owned(),
            source_roots: config
                .source_roots
                .iter()
                .map(|r| root_dir.join(r))
                .collect(),
            sim_roots: config.sim.roots,
            shims: config.shims,
            deterministic_markers: config.markers.deterministic,
            sinks: SinkDb::new(rules),
            trusted_modules: trusted,
            opaque_allow: config.opaque.allow,
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn source_roots(&self) -> &[PathBuf] {
        &self.source_roots
    }

    pub fn sim_roots(&self) -> &[String] {
        &self.sim_roots
    }

    pub fn sinks(&self) -> &SinkDb {
        &self.sinks
    }

    /// Prod mode is enabled by declaring at least one shim protocol: without
    /// one there is nothing to be "outside of".
    pub fn prod_enabled(&self) -> bool {
        !self.shims.is_empty()
    }

    /// Sim mode is enabled by declaring at least one simulation root module.
    pub fn sim_enabled(&self) -> bool {
        !self.sim_roots.is_empty()
    }

    /// Capabilities granted to implementations of `protocol`, unioned over
    /// every matching `[[python.shims]]` entry.
    pub fn grants_for_protocol(&self, protocol: &str) -> Vec<&str> {
        self.shims
            .iter()
            .filter(|shim| shim.protocol == protocol)
            .flat_map(|shim| shim.grants.iter().map(String::as_str))
            .collect()
    }

    pub fn is_shim_protocol(&self, protocol: &str) -> bool {
        self.shims.iter().any(|shim| shim.protocol == protocol)
    }

    pub fn is_deterministic_marker(&self, class: &str) -> bool {
        self.deterministic_markers.iter().any(|m| m == class)
    }

    /// A module is trusted if it, or any ancestor package of it, is listed.
    pub fn is_trusted_module(&self, module: &str) -> bool {
        self.trusted_modules
            .iter()
            .any(|t| module == t || module.starts_with(&format!("{t}.")))
    }

    pub fn is_opaque_allowed(&self, qualified: &str) -> bool {
        self.opaque_allow.iter().any(|a| a == qualified)
    }
}

/// Reads and parses one config file, returning only the `[python]` section.
/// `Ok(None)` means the file exists but declares no `[python]` section, in
/// which case the Python analysis is inert.
pub fn parse_file(path: &Path) -> Result<Option<PythonConfig>, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    parse_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_owned(),
        source,
    })
}

/// Parses config text through the full shared schema and keeps `[python]`.
pub fn parse_str(text: &str) -> Result<Option<PythonConfig>, toml::de::Error> {
    let config: trigpoint_config::Config = toml::from_str(text)?;
    Ok(config.python)
}

/// Walks up from `start` looking for `triglint.toml`.
pub fn discover(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(CONFIG_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

/// Locates the config the way triglint does: `$TRIGLINT_CONFIG` if set,
/// otherwise the nearest `triglint.toml` walking up from `start`.
pub fn locate(start: &Path) -> Option<PathBuf> {
    match env::var(CONFIG_ENV) {
        Ok(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => discover(start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
        # Keys belonging to the Rust linter's schema parse under the shared
        # strict schema alongside [python].
        builtin_sinks = false

        [sim]
        roots = ["sim_harness::main"]

        [[shims]]
        trait = "demo_lib::ClockShim"
        grants = ["time"]

        [python]
        source_roots = ["src"]
        builtin_sinks = true

        [python.sim]
        roots = ["myproj.sim.harness"]

        [[python.shims]]
        protocol = "myproj.shims.ClockShim"
        grants = ["time"]

        [[python.shims]]
        protocol = "myproj.shims.EntropyShim"
        grants = ["random", "hash_order"]

        [python.markers]
        deterministic = ["trigpoint_shims.DeterministicShim"]

        [[python.sinks]]
        capability = "time"
        calls = ["arrow.utcnow"]
        modules = ["pendulum"]

        [python.opaque]
        trusted_modules = ["attrs"]
        allow = ["myproj.plugins.load"]
    "#;

    fn resolved(text: &str) -> Resolved {
        let config = parse_str(text)
            .expect("config should parse")
            .expect("config should have a [python] section");
        Resolved::new(Path::new("/proj"), config)
    }

    #[test]
    fn full_schema_parses_alongside_rust_keys() {
        let config = resolved(FULL);
        assert_eq!(config.source_roots(), [PathBuf::from("/proj/src")]);
        assert_eq!(config.sim_roots(), ["myproj.sim.harness"]);
        assert!(config.prod_enabled());
        assert!(config.sim_enabled());
        assert_eq!(
            config.grants_for_protocol("myproj.shims.ClockShim"),
            vec!["time"]
        );
        assert_eq!(
            config.grants_for_protocol("myproj.shims.EntropyShim"),
            vec!["random", "hash_order"]
        );
        assert!(config.grants_for_protocol("myproj.shims.Nope").is_empty());
        assert!(config.is_deterministic_marker("trigpoint_shims.DeterministicShim"));
        assert!(config.is_trusted_module("attrs"));
        assert!(config.is_trusted_module("attrs.converters"));
        assert!(!config.is_trusted_module("attrsx"));
        assert!(config.is_opaque_allowed("myproj.plugins.load"));
    }

    #[test]
    fn no_python_section_is_inert() {
        assert!(parse_str("[sim]\nroots = []\n").expect("parses").is_none());
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = resolved("[python]\n");
        assert_eq!(config.source_roots(), [PathBuf::from("/proj/.")]);
        assert!(!config.prod_enabled());
        assert!(!config.sim_enabled());
        assert!(config.is_deterministic_marker("trigpoint_shims.DeterministicShim"));
        // Builtins are on by default.
        assert!(!config.sinks().is_empty());
    }

    #[test]
    fn unknown_python_keys_are_rejected() {
        let error = parse_str("[python]\nsource_rootz = []\n").expect_err("should reject typos");
        assert!(error.to_string().contains("source_rootz"), "{error}");
    }

    #[test]
    fn unknown_rust_keys_are_now_rejected_too() {
        // Under the split schemas this was silently ignored; the shared
        // schema validates the whole file for every consumer.
        assert!(parse_str("[nonsense]\nkey = 1\n").is_err());
    }

    #[test]
    fn builtin_sinks_can_be_disabled() {
        let config = resolved("[python]\nbuiltin_sinks = false\n");
        assert_eq!(config.sinks().len(), 0);
    }

    #[test]
    fn discovery_walks_up() {
        let base = env::temp_dir().join("trigpylint-test-discover");
        let nested = base.join("a").join("b");
        fs::create_dir_all(&nested).expect("create dirs");
        fs::write(base.join(CONFIG_FILE), "[python]\n").expect("write config");
        assert_eq!(discover(&nested), Some(base.join(CONFIG_FILE)));
        fs::remove_dir_all(&base).expect("cleanup");
    }
}
