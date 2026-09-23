//! Per-module binding tables and qualified-name resolution.
//!
//! The table answers one question: *what dotted name does this local name
//! stand for?* It is built from imports (absolute, aliased, `from`, relative),
//! module-level `NAME = dotted.name` aliases, and module-level `def`/`class`
//! statements (so a shim protocol defined in the module it is declared in
//! resolves to `<module>.<Class>`).
//!
//! Two deliberate over-approximations, both erring towards *more* findings:
//!
//! * Imports are collected from the whole module, not just its top level. A
//!   function-local `import random` therefore binds `random` for the module.
//!   Missing those would hide sinks; hoisting them can only surface more.
//! * A name that is assigned anywhere in the module is recorded as *shadowed*
//!   and stops resolving to a builtin, module-wide. This is scope-insensitive:
//!   a local named `open` in one function silences the `open` sink everywhere
//!   in that module. It is the documented precision boundary for builtins.

use std::collections::{BTreeMap, BTreeSet};

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, ExprContext, Parameter, Stmt};
use ruff_text_size::TextRange;

use super::modules::{Module, ModuleTable, resolve_relative};

/// A `from x import *` whose target could not be resolved to analyzed source.
#[derive(Debug, Clone)]
pub struct UnresolvedStar {
    pub target: String,
    pub range: TextRange,
}

/// What a resolved dotted name denotes, as far as the binding table can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    /// The name denotes a module object (an `import` target or a collected
    /// module). Attribute assignment on one of these is monkeypatching.
    Module,
    /// Anything else: a function, class, constant, or an attribute of a module.
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    pub qualified: String,
    pub kind: NameKind,
}

#[derive(Debug, Default)]
pub struct Bindings {
    /// Local name → dotted qualified name.
    names: BTreeMap<String, String>,
    /// Names assigned somewhere in the module: they no longer denote builtins.
    shadowed: BTreeSet<String>,
    /// Dotted names known to denote modules.
    modules: BTreeSet<String>,
    /// `from x import *` targets that are in analyzed source, pending merge.
    star_targets: Vec<String>,
    /// `from x import *` targets that are not, reported as holes.
    unresolved_stars: Vec<UnresolvedStar>,
}

impl Bindings {
    pub fn lookup(&self, name: &str) -> Option<&str> {
        self.names.get(name).map(String::as_str)
    }

    pub fn is_shadowed(&self, name: &str) -> bool {
        self.shadowed.contains(name)
    }

    pub fn is_module(&self, qualified: &str) -> bool {
        self.modules.contains(qualified)
    }

    pub fn unresolved_stars(&self) -> &[UnresolvedStar] {
        &self.unresolved_stars
    }

    /// Resolves an expression to the dotted name it denotes, if any.
    ///
    /// `Name` resolves through the table, or — when nothing in the module ever
    /// assigns it — to itself, which is how builtins such as `open` and `input`
    /// are matched. `Attribute` extends its base. Everything else (subscripts,
    /// calls, comprehensions) is unresolvable by construction.
    pub fn resolve(&self, expr: &Expr) -> Option<ResolvedName> {
        match expr {
            Expr::Name(name) => {
                let id = name.id.as_str();
                if let Some(qualified) = self.lookup(id) {
                    return Some(ResolvedName {
                        qualified: qualified.to_owned(),
                        kind: self.kind_of(qualified),
                    });
                }
                if self.is_shadowed(id) {
                    return None;
                }
                Some(ResolvedName {
                    qualified: id.to_owned(),
                    kind: self.kind_of(id),
                })
            }
            Expr::Attribute(attribute) => {
                let base = self.resolve(&attribute.value)?;
                let qualified = format!("{}.{}", base.qualified, attribute.attr.as_str());
                let kind = self.kind_of(&qualified);
                Some(ResolvedName { qualified, kind })
            }
            _ => None,
        }
    }

    fn kind_of(&self, qualified: &str) -> NameKind {
        if self.modules.contains(qualified) {
            NameKind::Module
        } else {
            NameKind::Value
        }
    }

    fn bind(&mut self, local: &str, qualified: String) {
        self.names.insert(local.to_owned(), qualified);
    }

    fn note_module(&mut self, qualified: &str) {
        // Importing `a.b.c` executes `a` and `a.b` too; all three are modules.
        let mut prefix = String::new();
        for part in qualified.split('.') {
            if !prefix.is_empty() {
                prefix.push('.');
            }
            prefix.push_str(part);
            self.modules.insert(prefix.clone());
        }
    }
}

/// Builds the binding table for one module. Star imports are recorded but not
/// merged; call [`merge_star_imports`] once every module has a table.
pub fn build(module: &Module, table: &ModuleTable) -> Bindings {
    let mut bindings = Bindings::default();
    for name in table.names() {
        bindings.modules.insert(name.to_owned());
    }

    let mut collector = Collector {
        bindings: &mut bindings,
        module,
        table,
    };
    collector.visit_body(&module.ast().body);

    // Module-level `NAME = dotted.name` aliases, resolved in source order
    // against the imports already in the table. Two passes catch one level of
    // alias chaining (`a = time.time; b = a`), which is as far as a
    // syntactic rule can honestly go.
    for _ in 0..2 {
        for stmt in &module.ast().body {
            let Stmt::Assign(assign) = stmt else { continue };
            let [Expr::Name(target)] = assign.targets.as_slice() else {
                continue;
            };
            let Some(resolved) = bindings.resolve(&assign.value) else {
                continue;
            };
            if resolved.qualified == target.id.as_str() {
                continue;
            }
            bindings.bind(target.id.as_str(), resolved.qualified);
        }
    }
    bindings
}

/// Resolves `from x import *` by copying `x`'s bindings into the importer,
/// iterating to a fixpoint so chains of star imports settle.
///
/// Names already bound locally win; a star import cannot displace an explicit
/// one. Targets outside analyzed source stay in `unresolved_stars` and are
/// reported as holes.
pub fn merge_star_imports(tables: &mut [Bindings], table: &ModuleTable) {
    for _ in 0..=tables.len() {
        let mut changed = false;
        for index in 0..tables.len() {
            let targets = tables[index].star_targets.clone();
            for target in targets {
                let Some(source) = table.id_of(&target) else {
                    continue;
                };
                if source.index() == index {
                    continue;
                }
                let incoming: Vec<(String, String)> = tables[source.index()]
                    .names
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (local, qualified) in incoming {
                    if let std::collections::btree_map::Entry::Vacant(slot) =
                        tables[index].names.entry(local)
                    {
                        slot.insert(qualified);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

struct Collector<'a> {
    bindings: &'a mut Bindings,
    module: &'a Module,
    table: &'a ModuleTable,
}

impl<'a> Collector<'a> {
    fn import(&mut self, stmt: &ruff_python_ast::StmtImport) {
        for alias in &stmt.names {
            let target = alias.name.as_str();
            self.bindings.note_module(target);
            match &alias.asname {
                // `import a.b as c` binds `c` to `a.b`.
                Some(asname) => self.bindings.bind(asname.as_str(), target.to_owned()),
                // `import a.b` binds only `a`.
                None => {
                    let head = target.split('.').next().unwrap_or(target);
                    self.bindings.bind(head, head.to_owned());
                }
            }
        }
    }

    fn import_from(&mut self, stmt: &ruff_python_ast::StmtImportFrom) {
        let base = if stmt.level == 0 {
            stmt.module.as_ref().map(|m| m.as_str().to_owned())
        } else {
            resolve_relative(
                self.module.package(),
                stmt.level,
                stmt.module
                    .as_ref()
                    .map(ruff_python_ast::Identifier::as_str),
            )
        };
        let Some(base) = base else { return };
        self.bindings.note_module(&base);
        for alias in &stmt.names {
            let name = alias.name.as_str();
            if name == "*" {
                if self.table.contains(&base) {
                    self.bindings.star_targets.push(base.clone());
                } else {
                    self.bindings.unresolved_stars.push(UnresolvedStar {
                        target: base.clone(),
                        range: stmt.range,
                    });
                }
                continue;
            }
            let qualified = format!("{base}.{name}");
            if self.table.contains(&qualified) {
                self.bindings.note_module(&qualified);
            }
            let local = alias.asname.as_ref().map_or(name, |a| a.as_str());
            self.bindings.bind(local, qualified);
        }
    }

    fn shadow(&mut self, name: &str) {
        self.bindings.shadowed.insert(name.to_owned());
    }

    fn shadow_target(&mut self, expr: &Expr) {
        match expr {
            Expr::Name(name) => self.shadow(name.id.as_str()),
            Expr::Tuple(tuple) => {
                for element in &tuple.elts {
                    self.shadow_target(element);
                }
            }
            Expr::List(list) => {
                for element in &list.elts {
                    self.shadow_target(element);
                }
            }
            Expr::Starred(starred) => self.shadow_target(&starred.value),
            _ => {}
        }
    }
}

impl<'a> Visitor<'a> for Collector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(import) => {
                self.import(import);
                return;
            }
            Stmt::ImportFrom(import) => {
                self.import_from(import);
                return;
            }
            Stmt::FunctionDef(function) => {
                self.shadow(function.name.as_str());
                self.bind_module_level(stmt, function.name.as_str());
            }
            Stmt::ClassDef(class) => {
                self.shadow(class.name.as_str());
                self.bind_module_level(stmt, class.name.as_str());
            }
            Stmt::Global(global) => {
                for name in &global.names {
                    self.shadow(name.as_str());
                }
            }
            Stmt::Nonlocal(nonlocal) => {
                for name in &nonlocal.names {
                    self.shadow(name.as_str());
                }
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_parameter(&mut self, parameter: &'a Parameter) {
        self.shadow(parameter.name.as_str());
        visitor::walk_parameter(self, parameter);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(name) = expr
            && matches!(name.ctx, ExprContext::Store | ExprContext::Del)
        {
            self.shadow(name.id.as_str());
        }
        if let Expr::Named(named) = expr {
            self.shadow_target(&named.target);
        }
        visitor::walk_expr(self, expr);
    }
}

impl<'a> Collector<'a> {
    /// Registers a top-level `def`/`class` as `<module>.<name>`, which is how
    /// a shim protocol declared in the module it is configured under resolves.
    fn bind_module_level(&mut self, stmt: &Stmt, name: &str) {
        if self
            .module
            .ast()
            .body
            .iter()
            .any(|top| std::ptr::eq(top, stmt))
        {
            let qualified = format!("{}.{}", self.module.name, name);
            self.bindings.bind(name, qualified);
        }
    }
}
