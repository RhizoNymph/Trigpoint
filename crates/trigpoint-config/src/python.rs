//! The `[python]` half of the `triglint.toml` schema.
//!
//! Schema only: the queryable `Resolved` view, the builtin Python sink
//! database, and config discovery anchored at a directory live in
//! `trigpoint-pylint`, which consumes these types. Keeping the schema here
//! means one strict definition of the whole file: a typo in either half is
//! an error for every consumer.

use std::path::PathBuf;

use serde::Deserialize;

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
