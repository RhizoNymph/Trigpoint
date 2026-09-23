//! The `triglint.toml` schema lives in the stable `trigpoint-config` crate
//! (`crates/trigpoint-config`) so the Python analysis can share it; this
//! module only re-exports it under the name the lint code already uses.

pub use trigpoint_config::*;
