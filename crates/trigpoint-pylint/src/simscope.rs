//! Sim mode: the import graph, its closure from the declared roots, and the
//! witness chains that explain how a sink got there.
//!
//! Reachability is over *module imports*, not calls: importing a module pulls
//! its whole surface into simulation scope even when only one function is
//! used. That is deliberately over-approximate, and is the Python counterpart
//! of triglint's "a collected vtable method counts even if never invoked".
//!
//! Inside the closure the rule is absolute: zero sinks, blessing or not. A
//! shim implementation marked deterministic that names one is reported as a
//! broken claim rather than excused.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

use crate::config::Resolved;
use crate::resolve::modules::resolve_relative;
use crate::resolve::{Module, ModuleId, ModuleTable, Program};

/// One module-level import edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    /// Absolute dotted module path.
    pub module: String,
    pub range: TextRange,
    /// Whether failing to resolve this target is a hole. `from a import b`
    /// makes `a` required and `a.b` optional: `b` may simply be a function.
    pub required: bool,
}

/// An import inside the closure that led somewhere the analysis cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedImport {
    pub module: ModuleId,
    pub target: String,
    pub range: TextRange,
}

/// The transitive import closure of the declared sim roots.
#[derive(Debug, Default)]
pub struct Closure {
    /// Modules in the closure, in discovery order.
    pub order: Vec<ModuleId>,
    /// Root → … → module, by module. Always non-empty for members of `order`.
    pub chains: BTreeMap<ModuleId, Vec<String>>,
    pub unresolved: Vec<UnresolvedImport>,
}

impl Closure {
    pub fn contains(&self, id: ModuleId) -> bool {
        self.chains.contains_key(&id)
    }

    /// The witness chain for a module, rendered as `root > middle > module`.
    pub fn witness(&self, id: ModuleId) -> String {
        self.chains
            .get(&id)
            .map(|chain| chain.join(" > "))
            .unwrap_or_default()
    }
}

/// Sim roots naming modules that were never collected. Failing loudly beats
/// silently checking nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingRoots(pub Vec<String>);

/// Walks the import graph outward from `config.sim_roots()`.
pub fn closure(program: &Program, config: &Resolved) -> Result<Closure, MissingRoots> {
    let table = &program.modules;
    let mut missing = Vec::new();
    let mut queue: VecDeque<ModuleId> = VecDeque::new();
    let mut closure = Closure::default();

    for root in config.sim_roots() {
        match table.id_of(root) {
            Some(id) => {
                if closure.chains.insert(id, vec![root.clone()]).is_none() {
                    closure.order.push(id);
                    queue.push_back(id);
                }
            }
            None => missing.push(root.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(MissingRoots(missing));
    }

    while let Some(id) = queue.pop_front() {
        let module = table.module(id);
        let chain = closure.chains[&id].clone();
        for target in import_targets(module) {
            for candidate in ancestry(&target.module) {
                let required = target.required && candidate == target.module;
                if let Some(next) = table.id_of(&candidate) {
                    if let std::collections::btree_map::Entry::Vacant(slot) =
                        closure.chains.entry(next)
                    {
                        let mut next_chain = chain.clone();
                        next_chain.push(candidate.clone());
                        slot.insert(next_chain);
                        closure.order.push(next);
                        queue.push_back(next);
                    }
                    continue;
                }
                if !required {
                    continue;
                }
                // A fenced module is already reported at the import statement
                // by the sim scan; do not also call it opaque.
                if config.sinks().match_module(&candidate).is_some() {
                    continue;
                }
                if config.is_trusted_module(&candidate) || config.is_opaque_allowed(&candidate) {
                    continue;
                }
                closure.unresolved.push(UnresolvedImport {
                    module: id,
                    target: candidate.clone(),
                    range: target.range,
                });
            }
        }
    }

    closure.unresolved.sort_by(|a, b| {
        (a.module, a.range.start(), &a.target).cmp(&(b.module, b.range.start(), &b.target))
    });
    closure.unresolved.dedup();
    Ok(closure)
}

/// `a.b.c` → `a`, `a.b`, `a.b.c`: importing a submodule executes its packages.
fn ancestry(module: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prefix = String::new();
    for part in module.split('.') {
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(part);
        out.push(prefix.clone());
    }
    out
}

/// Every import edge in a module, including imports nested inside functions —
/// they execute when the function runs, which is inside the simulation.
pub fn import_targets(module: &Module) -> Vec<ImportTarget> {
    let mut collector = TargetCollector {
        module,
        targets: Vec::new(),
        seen: BTreeSet::new(),
    };
    collector.visit_body(&module.ast().body);
    collector.targets
}

struct TargetCollector<'a> {
    module: &'a Module,
    targets: Vec<ImportTarget>,
    seen: BTreeSet<(String, u32)>,
}

impl<'a> TargetCollector<'a> {
    fn push(&mut self, module: String, range: TextRange, required: bool) {
        if self.seen.insert((module.clone(), range.start().into())) {
            self.targets.push(ImportTarget {
                module,
                range,
                required,
            });
        }
    }
}

impl<'a> Visitor<'a> for TargetCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    self.push(alias.name.as_str().to_owned(), stmt.range(), true);
                }
            }
            Stmt::ImportFrom(import) => {
                let base = if import.level == 0 {
                    import.module.as_ref().map(|m| m.as_str().to_owned())
                } else {
                    resolve_relative(
                        self.module.package(),
                        import.level,
                        import
                            .module
                            .as_ref()
                            .map(ruff_python_ast::Identifier::as_str),
                    )
                };
                let Some(base) = base else { return };
                self.push(base.clone(), stmt.range(), true);
                for alias in &import.names {
                    let name = alias.name.as_str();
                    if name != "*" {
                        self.push(format!("{base}.{name}"), stmt.range(), false);
                    }
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, _expr: &'a Expr) {}
}

/// Modules in the closure, paired with the witness chain that reached them.
pub fn closure_modules<'a>(
    table: &'a ModuleTable,
    closure: &'a Closure,
) -> impl Iterator<Item = (&'a Module, String)> {
    closure
        .order
        .iter()
        .map(move |id| (table.module(*id), closure.witness(*id)))
}
