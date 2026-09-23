//! Module mapping, binding tables, and qualified-name resolution.
//!
//! Split into two focused submodules:
//!
//! * [`modules`] — `source_roots` → dotted module paths, parsing, and
//!   relative-import arithmetic.
//! * [`bindings`] — per-module binding tables and the `Expr` → dotted-name
//!   resolution they power.

pub mod bindings;
pub mod modules;

use std::path::Path;

use thiserror::Error;

pub use bindings::{Bindings, NameKind, ResolvedName};
pub use modules::{Module, ModuleId, ModuleTable};

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error(transparent)]
    Collect(#[from] modules::CollectError),
}

/// Everything the checks need about a project's Python sources: the module
/// table plus one binding table per module, indexed by [`ModuleId`].
pub struct Program {
    pub modules: ModuleTable,
    bindings: Vec<Bindings>,
}

impl Program {
    /// Collects and resolves every module under `source_roots`.
    pub fn load(
        display_root: &Path,
        source_roots: &[std::path::PathBuf],
    ) -> Result<Self, ResolveError> {
        let modules = modules::collect(display_root, source_roots)?;
        let mut bindings: Vec<Bindings> = modules
            .iter()
            .map(|module| bindings::build(module, &modules))
            .collect();
        bindings::merge_star_imports(&mut bindings, &modules);
        Ok(Self { modules, bindings })
    }

    pub fn bindings(&self, id: ModuleId) -> &Bindings {
        &self.bindings[id.index()]
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Module, &Bindings)> {
        self.modules
            .iter()
            .map(move |module| (module, &self.bindings[module.id.index()]))
    }
}
