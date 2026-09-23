//! Module collection: `source_roots` → dotted module paths.
//!
//! `src/myproj/sim/harness.py` under the source root `src` becomes
//! `myproj.sim.harness`; `src/myproj/__init__.py` becomes `myproj` and is
//! flagged as a package, which is what relative-import resolution keys on.
//! Directories without `__init__.py` still contribute packages (PEP 420
//! namespace packages are ordinary in modern Python), so the flag records what
//! a file *is*, not whether its parents opted in.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ruff_python_ast::ModModule;
use ruff_python_parser::{ParseError, Parsed};
use thiserror::Error;

/// Directory names never walked: virtualenvs, caches and build output cannot
/// contribute first-party modules and are expensive to parse.
const SKIPPED_DIRS: &[&str] = &[
    "__pycache__",
    "node_modules",
    "site-packages",
    "venv",
    ".venv",
    "build",
    "dist",
    ".git",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    "target",
];

#[derive(Debug, Error)]
pub enum CollectError {
    #[error("failed to read source root {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Index into a [`ModuleTable`]. Only the table hands these out, so a
/// `ModuleId` always denotes a collected module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId(usize);

impl ModuleId {
    pub fn index(self) -> usize {
        self.0
    }
}

pub struct Module {
    pub id: ModuleId,
    /// Dotted module path, e.g. `myproj.sim.harness`.
    pub name: String,
    /// Absolute path to the `.py` file.
    pub path: PathBuf,
    /// Path as shown in diagnostics: relative to the config directory, with
    /// forward slashes.
    pub display_path: String,
    /// True for `__init__.py`: the file *is* its package.
    pub is_package: bool,
    pub source: String,
    pub parsed: Parsed<ModModule>,
}

impl Module {
    pub fn ast(&self) -> &ModModule {
        self.parsed.syntax()
    }

    /// The package a relative import inside this module is relative to.
    pub fn package(&self) -> &str {
        if self.is_package {
            &self.name
        } else {
            match self.name.rfind('.') {
                Some(dot) => &self.name[..dot],
                None => "",
            }
        }
    }
}

/// A file that could not be parsed. Reported as an `unresolved` warning rather
/// than silently skipped: an unparsed module is scope the analysis cannot see.
pub struct UnparsedModule {
    pub path: PathBuf,
    pub display_path: String,
    pub error: ParseError,
}

pub struct ModuleTable {
    modules: Vec<Module>,
    by_name: BTreeMap<String, ModuleId>,
    unparsed: Vec<UnparsedModule>,
}

impl ModuleTable {
    pub fn get(&self, name: &str) -> Option<&Module> {
        self.by_name.get(name).map(|id| &self.modules[id.0])
    }

    pub fn id_of(&self, name: &str) -> Option<ModuleId> {
        self.by_name.get(name).copied()
    }

    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.0]
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Module> {
        self.modules.iter()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn unparsed(&self) -> &[UnparsedModule] {
        &self.unparsed
    }

    /// Every dotted name that denotes a collected module or one of its
    /// ancestor packages.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}

/// Walks every source root and parses every importable `.py` file.
///
/// When two roots map a file to the same module path the first root wins,
/// matching Python's own `sys.path` precedence.
pub fn collect(display_root: &Path, source_roots: &[PathBuf]) -> Result<ModuleTable, CollectError> {
    let mut files: Vec<(String, PathBuf, bool)> = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for root in source_roots {
        let mut found = Vec::new();
        walk(root, &mut Vec::new(), &mut found)?;
        for (name, path, is_package) in found {
            if seen.insert(name.clone(), ()).is_none() {
                files.push((name, path, is_package));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut modules = Vec::new();
    let mut by_name = BTreeMap::new();
    let mut unparsed = Vec::new();
    for (name, path, is_package) in files {
        let source = fs::read_to_string(&path).map_err(|source| CollectError::ReadFile {
            path: path.clone(),
            source,
        })?;
        let display_path = display_path(display_root, &path);
        match ruff_python_parser::parse_module(&source) {
            Ok(parsed) => {
                let id = ModuleId(modules.len());
                by_name.insert(name.clone(), id);
                modules.push(Module {
                    id,
                    name,
                    path,
                    display_path,
                    is_package,
                    source,
                    parsed,
                });
            }
            Err(error) => unparsed.push(UnparsedModule {
                path,
                display_path,
                error,
            }),
        }
    }
    Ok(ModuleTable {
        modules,
        by_name,
        unparsed,
    })
}

/// Renders `path` relative to `base` with forward slashes, falling back to the
/// absolute path when it lies outside `base`.
pub fn display_path(base: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(base).unwrap_or(path);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn walk(
    dir: &Path,
    prefix: &mut Vec<String>,
    out: &mut Vec<(String, PathBuf, bool)>,
) -> Result<(), CollectError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // A configured source root that does not exist is the caller's
        // problem to report; a vanished subdirectory is not worth failing on.
        Err(source) if prefix.is_empty() => {
            return Err(CollectError::ReadDir {
                path: dir.to_owned(),
                source,
            });
        }
        Err(_) => return Ok(()),
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| CollectError::ReadDir {
            path: dir.to_owned(),
            source,
        })?;
        children.push(entry.path());
    }
    children.sort();

    for child in children {
        let Some(file_name) = child.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if child.is_dir() {
            if SKIPPED_DIRS.contains(&file_name) || file_name.starts_with('.') {
                continue;
            }
            if !is_identifier(file_name) {
                continue;
            }
            prefix.push(file_name.to_owned());
            walk(&child, prefix, out)?;
            prefix.pop();
            continue;
        }
        let Some(stem) = file_name.strip_suffix(".py") else {
            continue;
        };
        if stem == "__init__" {
            if prefix.is_empty() {
                // An `__init__.py` directly in a source root has no package
                // name to take; it is not importable as anything.
                continue;
            }
            out.push((prefix.join("."), child, true));
        } else if is_identifier(stem) {
            let mut parts = prefix.clone();
            parts.push(stem.to_owned());
            out.push((parts.join("."), child, false));
        }
    }
    Ok(())
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Resolves a relative import to an absolute dotted module path.
///
/// `package` is the importing module's package (see [`Module::package`]),
/// `level` the number of leading dots, `tail` the dotted name after them.
/// Returns `None` when the dots walk past the top-level package.
pub fn resolve_relative(package: &str, level: u32, tail: Option<&str>) -> Option<String> {
    let mut parts: Vec<&str> = if package.is_empty() {
        Vec::new()
    } else {
        package.split('.').collect()
    };
    // One dot means "this package"; each extra dot pops one level.
    for _ in 1..level {
        parts.pop()?;
    }
    if level > 0 && parts.is_empty() && package.is_empty() {
        return None;
    }
    let mut base = parts.join(".");
    if let Some(tail) = tail {
        if base.is_empty() {
            base = tail.to_owned();
        } else {
            base = format!("{base}.{tail}");
        }
    }
    if base.is_empty() { None } else { Some(base) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_gate_module_names() {
        assert!(is_identifier("harness"));
        assert!(is_identifier("_private"));
        assert!(!is_identifier("my-module"));
        assert!(!is_identifier("2fast"));
    }

    #[test]
    fn relative_imports_resolve_against_the_package() {
        // Inside `pkg.sub.mod`, whose package is `pkg.sub`.
        assert_eq!(
            resolve_relative("pkg.sub", 1, Some("sibling")).as_deref(),
            Some("pkg.sub.sibling")
        );
        assert_eq!(
            resolve_relative("pkg.sub", 1, None).as_deref(),
            Some("pkg.sub")
        );
        assert_eq!(
            resolve_relative("pkg.sub", 2, Some("other")).as_deref(),
            Some("pkg.other")
        );
        assert_eq!(resolve_relative("pkg.sub", 3, None), None);
        assert_eq!(resolve_relative("", 1, Some("x")), None);
    }

    #[test]
    fn display_paths_are_relative_and_slashed() {
        assert_eq!(
            display_path(Path::new("/proj"), Path::new("/proj/src/pkg/mod.py")),
            "src/pkg/mod.py"
        );
    }
}
