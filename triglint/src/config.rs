//! `triglint.toml` schema, discovery, and the builtin sink database.
//!
//! This module is deliberately free of rustc types so the configuration
//! contract can be understood (and eventually unit-tested) without a
//! nightly compiler in hand.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Environment variable overriding config discovery. Used by UI tests and by
/// orchestration that knows exactly which config applies.
pub const CONFIG_ENV: &str = "TRIGLINT_CONFIG";

/// File name discovered by walking up from the linted crate's manifest dir.
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

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub sim: Sim,
    #[serde(default)]
    pub markers: Markers,
    /// Merge the builtin sink database into `sinks`. Default true.
    #[serde(default = "default_true")]
    pub builtin_sinks: bool,
    #[serde(default)]
    pub sinks: Vec<SinkSpec>,
    /// Shim traits and the capabilities their impls are granted. Presence of
    /// any entry enables prod mode (the `shim_nondeterminism` lint).
    #[serde(default)]
    pub shims: Vec<ShimSpec>,
    #[serde(default)]
    pub prod: Prod,
    #[serde(default)]
    pub opaque: Opaque,
}

/// A shim trait declaration: impls of `trait` may touch the `grants`
/// capabilities directly. Impls whose self type is marked deterministic
/// receive no grants — a sim impl must not touch anything.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShimSpec {
    /// Def-path of the shim trait, matched with and without the leading
    /// crate name.
    #[serde(rename = "trait")]
    pub trait_path: String,
    /// Capability labels this shim's impls may touch.
    pub grants: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prod {
    /// Also flag nondeterministic *types* (e.g. RandomState) appearing in
    /// unblessed bodies. Off by default: shared prod code often uses default
    /// hash maps deliberately, and sim mode already flags them when they
    /// reach a simulation root.
    #[serde(default)]
    pub type_sinks: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sim {
    /// Fully-qualified def-paths of non-generic functions to root the
    /// whole-program analysis at. Matched with and without the leading
    /// crate name.
    #[serde(default)]
    pub roots: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Markers {
    /// Marker traits declaring "this impl claims determinism".
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

fn default_deterministic_markers() -> Vec<String> {
    vec!["trigpoint_shims::DeterministicShim".to_owned()]
}

fn default_true() -> bool {
    true
}

/// One sink declaration: any call edge matching it carries `capability`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkSpec {
    /// Free-form capability label shown in diagnostics (e.g. "time").
    pub capability: String,
    /// Exact def-path matches (inherent form `Ty::method` or trait-impl form
    /// `<Ty as Trait>::method`).
    #[serde(default)]
    pub paths: Vec<String>,
    /// Def-path prefix matches on a `::` boundary (e.g. "std::fs").
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// Whole-crate fences: any call edge into the crate is a sink.
    #[serde(default)]
    pub crates: Vec<String>,
    /// Type sinks: ADT def-paths whose mere presence in a reachable
    /// function's generic arguments carries the capability. Robust against
    /// MIR inlining inside std, which erases call edges to constructors
    /// (e.g. `RandomState::new` inlined into `HashMap::new`).
    #[serde(default)]
    pub types: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opaque {
    /// Crates whose missing-MIR bodies are trusted silently, in addition to
    /// the builtin trusted set.
    #[serde(default)]
    pub trusted_crates: Vec<String>,
    /// Def-paths permitted to be opaque (missing MIR or foreign) without a
    /// diagnostic.
    #[serde(default)]
    pub allow: Vec<String>,
}

/// Crates trusted to be opaque by default. Sinks still match on the call
/// edge, so trusting a crate never masks a declared sink inside it.
pub const BUILTIN_TRUSTED_CRATES: &[&str] = &[
    "core",
    "alloc",
    "std",
    "proc_macro",
    "panic_unwind",
    "panic_abort",
    "unwind",
    "compiler_builtins",
    "rustc_std_workspace_core",
    "rustc_std_workspace_alloc",
    // std's hash-map implementation: its nondeterminism enters exclusively
    // through the hasher type parameter, which the RandomState type sink
    // catches structurally.
    "hashbrown",
    "trigpoint_shims",
];

fn sink(capability: &str, paths: &[&str], prefixes: &[&str], crates: &[&str]) -> SinkSpec {
    SinkSpec {
        capability: capability.to_owned(),
        paths: paths.iter().map(|s| (*s).to_owned()).collect(),
        prefixes: prefixes.iter().map(|s| (*s).to_owned()).collect(),
        crates: crates.iter().map(|s| (*s).to_owned()).collect(),
        types: Vec::new(),
    }
}

/// The builtin sink database, defined at the std/libc boundary so std's own
/// MIR is never required to catch its nondeterministic API surface.
pub fn builtin_sinks() -> Vec<SinkSpec> {
    vec![
        sink(
            "time",
            &["std::time::Instant::now", "std::time::SystemTime::now"],
            &[],
            &[],
        ),
        SinkSpec {
            capability: "random".to_owned(),
            paths: vec![
                // Belt and braces alongside the type sink below; these call
                // edges are usually inlined away inside std's shipped MIR,
                // but they fire when user code calls them directly.
                "std::hash::RandomState::new".to_owned(),
                "std::hash::random::RandomState::new".to_owned(),
                "std::collections::hash_map::RandomState::new".to_owned(),
            ],
            prefixes: vec![],
            crates: ["getrandom", "rand", "rand_core", "rand_chacha", "fastrand"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            // The load-bearing detection: any reachable instance whose
            // generic arguments mention RandomState (HashMap/HashSet default
            // hasher) carries process-random iteration order.
            types: vec![
                "std::hash::RandomState".to_owned(),
                "std::hash::random::RandomState".to_owned(),
                "std::collections::hash_map::RandomState".to_owned(),
            ],
        },
        sink(
            "thread",
            &[
                "std::thread::spawn",
                "std::thread::sleep",
                "std::thread::yield_now",
                "std::thread::park",
                "std::thread::park_timeout",
                "std::thread::Builder::spawn",
            ],
            &[],
            &[],
        ),
        sink("fs", &[], &["std::fs"], &[]),
        sink("net", &[], &["std::net"], &[]),
        sink("env", &[], &["std::env"], &[]),
        sink("process", &[], &["std::process"], &[]),
        sink("io", &["std::io::stdin"], &[], &[]),
    ]
}

/// A resolved, queryable view of the config.
pub struct Resolved {
    pub roots: Vec<String>,
    pub deterministic_markers: Vec<String>,
    sinks: Vec<SinkSpec>,
    shims: Vec<ShimSpec>,
    prod_type_sinks: bool,
    trusted_crates: Vec<String>,
    opaque_allow: Vec<String>,
}

impl Resolved {
    pub fn new(config: Config) -> Self {
        let mut sinks = if config.builtin_sinks {
            builtin_sinks()
        } else {
            Vec::new()
        };
        sinks.extend(config.sinks);
        let mut trusted_crates: Vec<String> = BUILTIN_TRUSTED_CRATES
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        trusted_crates.extend(config.opaque.trusted_crates);
        Self {
            roots: config.sim.roots,
            deterministic_markers: config.markers.deterministic,
            sinks,
            shims: config.shims,
            prod_type_sinks: config.prod.type_sinks,
            trusted_crates,
            opaque_allow: config.opaque.allow,
        }
    }

    /// Prod mode is enabled by declaring at least one shim: without shim
    /// declarations there is nothing to be "outside of".
    pub fn prod_enabled(&self) -> bool {
        !self.shims.is_empty()
    }

    pub fn prod_type_sinks(&self) -> bool {
        self.prod_type_sinks
    }

    /// Capabilities granted to impls of a trait whose def-path (any
    /// rendering) matches a `[[shims]]` entry.
    pub fn grants_for_trait(&self, trait_paths: &[&str]) -> Vec<&str> {
        self.shims
            .iter()
            .filter(|shim| trait_paths.iter().any(|p| *p == shim.trait_path))
            .flat_map(|shim| shim.grants.iter().map(String::as_str))
            .collect()
    }

    /// Matches a callee against the sink database. `paths` should contain
    /// every rendering of the def-path (with and without a leading crate
    /// name). Returns the capability label on a hit.
    pub fn match_sink(&self, crate_name: &str, paths: &[&str]) -> Option<&str> {
        for spec in &self.sinks {
            if spec.crates.iter().any(|c| c == crate_name) {
                return Some(&spec.capability);
            }
            for path in paths {
                if spec.paths.iter().any(|p| p == path) {
                    return Some(&spec.capability);
                }
                if spec.prefixes.iter().any(|prefix| {
                    path.strip_prefix(prefix.as_str())
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
                }) {
                    return Some(&spec.capability);
                }
            }
        }
        None
    }

    /// Matches an ADT's def-path renderings against the type-sink database.
    pub fn match_type_sink(&self, paths: &[&str]) -> Option<&str> {
        for spec in &self.sinks {
            if spec
                .types
                .iter()
                .any(|t| paths.iter().any(|p| p == t))
            {
                return Some(&spec.capability);
            }
        }
        None
    }

    pub fn is_trusted_crate(&self, crate_name: &str) -> bool {
        self.trusted_crates.iter().any(|c| c == crate_name)
    }

    pub fn is_opaque_allowed(&self, paths: &[&str]) -> bool {
        self.opaque_allow
            .iter()
            .any(|allowed| paths.iter().any(|p| p == allowed))
    }

    /// Root and marker entries match a def rendered with or without its
    /// leading crate name.
    pub fn matches_root(&self, paths: &[&str]) -> bool {
        self.roots
            .iter()
            .any(|root| paths.iter().any(|p| p == root))
    }

    pub fn matches_marker(&self, paths: &[&str]) -> bool {
        self.deterministic_markers
            .iter()
            .any(|m| paths.iter().any(|p| p == m))
    }
}

/// Loads the config: `$TRIGLINT_CONFIG` if set, otherwise the nearest
/// `triglint.toml` walking up from `$CARGO_MANIFEST_DIR`. `Ok(None)` means
/// "no config anywhere": the lint stays inert.
pub fn load() -> Result<Option<(PathBuf, Config)>, ConfigError> {
    if let Ok(path) = env::var(CONFIG_ENV) {
        let path = PathBuf::from(path);
        return parse(&path).map(|config| Some((path, config)));
    }
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return Ok(None);
    };
    let mut dir: &Path = Path::new(&manifest_dir);
    loop {
        let candidate = dir.join(CONFIG_FILE);
        if candidate.is_file() {
            return parse(&candidate).map(|config| Some((candidate, config)));
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(None),
        }
    }
}

fn parse(path: &Path) -> Result<Config, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(toml_text: &str) -> Resolved {
        Resolved::new(toml::from_str(toml_text).expect("config should parse"))
    }

    #[test]
    fn full_schema_parses() {
        let config = resolved(
            r#"
            # Top-level keys must precede any [table] header in TOML.
            builtin_sinks = true

            [sim]
            roots = ["sim_harness::main"]

            [markers]
            deterministic = ["trigpoint_shims::DeterministicShim"]

            [[sinks]]
            capability = "time"
            paths = ["chrono::Utc::now"]
            prefixes = ["some::module"]
            crates = ["chrono"]
            types = ["some::NondetType"]

            [[shims]]
            trait = "demo_lib::ClockShim"
            grants = ["time"]

            [prod]
            type_sinks = true

            [opaque]
            trusted_crates = ["my_vetted_crate"]
            allow = ["ffi_mod::checked_fn"]
            "#,
        );
        assert!(config.matches_root(&["sim_harness::main"]));
        assert!(config.prod_enabled());
        assert!(config.prod_type_sinks());
        assert_eq!(
            config.grants_for_trait(&["demo_lib::ClockShim"]),
            vec!["time"]
        );
        assert!(config.grants_for_trait(&["other::Trait"]).is_empty());
        assert_eq!(config.match_sink("chrono", &[]), Some("time"));
        assert_eq!(
            config.match_sink("mycrate", &["chrono::Utc::now"]),
            Some("time")
        );
        assert_eq!(
            config.match_type_sink(&["some::NondetType"]),
            Some("time")
        );
        assert!(config.is_trusted_crate("my_vetted_crate"));
        assert!(config.is_opaque_allowed(&["ffi_mod::checked_fn"]));
    }

    #[test]
    fn builtin_sinks_cover_std_surface() {
        let config = resolved("");
        assert_eq!(
            config.match_sink("std", &["std::time::Instant::now"]),
            Some("time")
        );
        assert_eq!(config.match_sink("rand", &[]), Some("random"));
        // Prefix match respects :: boundaries.
        assert_eq!(
            config.match_sink("std", &["std::fs::read_to_string"]),
            Some("fs")
        );
        assert_eq!(config.match_sink("std", &["std::fsx::not_fs"]), None);
        // The RandomState type sink is present in every rendering std has
        // used for it.
        assert_eq!(
            config.match_type_sink(&["std::hash::random::RandomState"]),
            Some("random")
        );
        // Prod mode is off without shim declarations.
        assert!(!config.prod_enabled());
        assert!(!config.prod_type_sinks());
    }

    #[test]
    fn builtin_sinks_can_be_disabled() {
        let config = resolved("builtin_sinks = false");
        assert_eq!(config.match_sink("std", &["std::time::Instant::now"]), None);
        assert_eq!(config.match_type_sink(&["std::hash::RandomState"]), None);
    }

    #[test]
    fn default_marker_is_trigpoint_shims() {
        let config = resolved("");
        assert!(config.matches_marker(&["trigpoint_shims::DeterministicShim"]));
    }
}
