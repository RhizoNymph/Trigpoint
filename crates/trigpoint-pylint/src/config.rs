//! The `[python]` section of `triglint.toml`: schema, discovery, resolution.
//!
//! # Temporary home
//!
//! This module is a **temporary** home for the Python half of the
//! `triglint.toml` schema. The sibling `feat/shared-config` workstream is
//! extracting triglint's Rust schema (`triglint/src/config.rs`) into a shared
//! stable crate `crates/trigpoint-config`; when that lands, everything in this
//! file merges into it and `trigpoint-pylint` depends on the shared crate
//! instead. Until then this module parses the same file *tolerantly*: it reads
//! only `[python]` and ignores every other top-level key, so the two schemas
//! can coexist in one file while they live in two crates.
//!
//! Discovery mirrors triglint exactly: `$TRIGLINT_CONFIG` if set, otherwise the
//! nearest `triglint.toml` walking up from a start directory.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::sinks::{SinkDb, SinkRule, builtin_sinks, builtin_trusted_modules};

/// Environment variable overriding config discovery.
pub const CONFIG_ENV: &str = "TRIGLINT_CONFIG";

/// File name discovered by walking up from the start directory.
pub const CONFIG_FILE: &str = "triglint.toml";

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

/// Tolerant view of `triglint.toml`. Unknown top-level keys belong to the Rust
/// linter's schema and are deliberately ignored here (no `deny_unknown_fields`
/// at this level); the `[python]` table itself is strict.
#[derive(Debug, Default, Deserialize)]
struct TriglintFile {
    #[serde(default)]
    python: Option<PythonConfig>,
}

/// The `[python]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonConfig {
    /// Import-resolution roots, relative to the config file's directory.
    #[serde(default = "default_source_roots")]
    pub source_roots: Vec<PathBuf>,
    #[serde(default)]
    pub sim: SimSection,
    /// Shim protocols and the capabilities their implementations are granted.
    /// A non-empty list enables prod mode.
    #[serde(default)]
    pub shims: Vec<ShimSpec>,
    #[serde(default)]
    pub markers: Markers,
    /// Merge the builtin sink database into `sinks`. Default true.
    #[serde(default = "default_true")]
    pub builtin_sinks: bool,
    #[serde(default)]
    pub sinks: Vec<SinkSpec>,
    #[serde(default)]
    pub opaque: Opaque,
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            source_roots: default_source_roots(),
            sim: SimSection::default(),
            shims: Vec::new(),
            markers: Markers::default(),
            builtin_sinks: true,
            sinks: Vec::new(),
            opaque: Opaque::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSection {
    /// Dotted module paths of simulation harness entry modules. A non-empty
    /// list enables sim mode.
    #[serde(default)]
    pub roots: Vec<String>,
}

/// `[[python.shims]]`: implementations of `protocol` may name sinks carrying
/// the `grants` capabilities.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShimSpec {
    /// Qualified class name of the shim protocol, e.g. `myproj.shims.ClockShim`.
    pub protocol: String,
    pub grants: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Markers {
    /// Qualified class names whose presence in a class's bases declares
    /// "this implementation claims determinism" — it receives no grants.
    #[serde(default = "default_deterministic_markers")]
    pub deterministic: Vec<String>,
}

impl Default for Markers {
    fn default() -> Self {
        Self {
            deterministic: default_deterministic_markers(),
        }
    }
}

/// `[[python.sinks]]`: extra sinks merged with the builtin database.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkSpec {
    /// Free-form capability label shown in diagnostics (e.g. "time").
    pub capability: String,
    /// Exact qualified-name matches, e.g. `arrow.utcnow`.
    #[serde(default)]
    pub calls: Vec<String>,
    /// Whole-module fences: any name under the module (and importing it inside
    /// sim scope) is a sink.
    #[serde(default)]
    pub modules: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opaque {
    /// Modules that may be imported inside sim scope without analyzable
    /// source, in addition to the builtin trusted set.
    #[serde(default)]
    pub trusted_modules: Vec<String>,
    /// Qualified names permitted to be reached dynamically/opaquely without an
    /// `unresolved` warning.
    #[serde(default)]
    pub allow: Vec<String>,
}

fn default_source_roots() -> Vec<PathBuf> {
    vec![PathBuf::from(".")]
}

fn default_deterministic_markers() -> Vec<String> {
    vec!["trigpoint_shims.DeterministicShim".to_owned()]
}

fn default_true() -> bool {
    true
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

/// Parses config text, ignoring everything outside `[python]`.
pub fn parse_str(text: &str) -> Result<Option<PythonConfig>, toml::de::Error> {
    let file: TriglintFile = toml::from_str(text)?;
    Ok(file.python)
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
        # Keys belonging to the Rust linter's schema must be ignored here.
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
    fn full_schema_parses_and_ignores_rust_keys() {
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
