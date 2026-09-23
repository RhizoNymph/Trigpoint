//! Blessing resolution and the per-module sink/hole scan.
//!
//! Prod mode's rule: a nondeterminism sink may only be *named* — called or
//! merely referenced — inside a class whose resolved bases include a declared
//! shim protocol granting the capability. Blessing is lexical: a function walks
//! outward through nested `def`s, lambdas and comprehensions to the nearest
//! enclosing `class`, which is the Python analogue of triglint's
//! `non_closure_owner`. A deterministic marker among the bases annuls every
//! grant, because a simulation implementation must not touch anything.
//!
//! The same scanner drives sim mode ([`Policy::Sim`]), where the rule is
//! simply "zero sinks, whatever the blessing" and the extra syntactic
//! `hash_order` heuristic switches on. Keeping one walker means the two modes
//! can never disagree about what counts as naming a sink.
//!
//! # Deviation from the design document
//!
//! The design's step 3c says module fences "also match the `import` statement
//! itself". That is applied in **sim mode only**. In prod mode the import
//! statement lives at module level, where no class can bless it, so reporting
//! it would make a shim implementation module impossible to write — the very
//! module that is *supposed* to import `time` could never do so. Prod mode
//! therefore reports at the use sites (which, by the reify rule, includes bare
//! references), and sim mode reports at the import.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, ExprContext, Stmt, StmtClassDef};
use ruff_text_size::{Ranged, TextRange};

use crate::config::Resolved;
use crate::resolve::bindings::NameKind;
use crate::resolve::{Bindings, Module};
use crate::sinks::{SinkKind, Usage};

/// Which rule the scan enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Sinks are allowed inside a blessing that covers the capability.
    Prod,
    /// No sinks at all; imports of fenced modules and the `hash_order`
    /// heuristic count too.
    Sim,
}

/// The blessing state of the nearest enclosing class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blessing {
    /// Not inside any class implementing a declared shim protocol.
    None,
    /// Inside a class whose bases include a deterministic marker: no grants.
    Marked { class: String },
    /// Inside a shim implementation with these grants.
    Shim {
        protocol: String,
        grants: Vec<String>,
    },
}

impl Blessing {
    fn covers(&self, capability: &str) -> bool {
        match self {
            Blessing::Shim { grants, .. } => grants.iter().any(|g| g == capability),
            Blessing::None | Blessing::Marked { .. } => false,
        }
    }

    fn context(&self) -> Context {
        match self {
            Blessing::None => Context::Unblessed,
            Blessing::Marked { class } => Context::MarkedDeterministic {
                class: class.clone(),
            },
            Blessing::Shim { protocol, grants } => Context::GrantsMissing {
                protocol: protocol.clone(),
                grants: grants.clone(),
            },
        }
    }
}

/// Why the site was not allowed to name the sink. Mirrors triglint's prod
/// diagnostic taxonomy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    Unblessed,
    MarkedDeterministic {
        class: String,
    },
    GrantsMissing {
        protocol: String,
        grants: Vec<String>,
    },
}

/// Something the analysis cannot see through, reported rather than assumed
/// safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hole {
    /// `importlib.*` or `__import__`.
    DynamicImport { name: String },
    /// `eval` / `exec`.
    DynamicEval { name: String },
    /// `getattr`/`setattr`/`delattr` with a non-literal attribute on a module.
    DynamicAttribute { function: String, module: String },
    /// Assignment to an attribute of an imported module.
    Monkeypatch { target: String },
    /// `from x import *` where `x` is not analyzed source.
    StarImport { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingKind {
    /// A sink named at this site.
    Sink {
        capability: String,
        /// The resolved qualified name as written.
        name: String,
        /// The database entry it matched.
        matched: SinkKind,
        context: Context,
    },
    /// An `import` of a fenced module inside sim scope.
    ImportFence {
        capability: String,
        module: String,
    },
    /// The syntactic set-construction heuristic (sim scope only).
    HashOrder {
        construct: &'static str,
        context: Context,
    },
    Hole(Hole),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    pub range: TextRange,
}

/// Scans one module under `policy`.
pub fn scan(
    module: &Module,
    bindings: &Bindings,
    config: &Resolved,
    policy: Policy,
) -> Vec<Finding> {
    let mut scanner = Scanner {
        bindings,
        config,
        policy,
        blessing: Blessing::None,
        findings: Vec::new(),
    };
    for star in bindings.unresolved_stars() {
        scanner.findings.push(Finding {
            kind: FindingKind::Hole(Hole::StarImport {
                target: star.target.clone(),
            }),
            range: star.range,
        });
    }
    scanner.visit_body(&module.ast().body);
    scanner
        .findings
        .sort_by_key(|f| (f.range.start(), f.range.end()));
    scanner.findings
}

/// Resolves a class statement's bases to a blessing.
pub fn blessing_of(class: &StmtClassDef, bindings: &Bindings, config: &Resolved) -> Blessing {
    let mut grants: Vec<String> = Vec::new();
    let mut protocol: Option<String> = None;
    let Some(arguments) = class.arguments.as_ref() else {
        return Blessing::None;
    };
    for base in arguments.args.iter() {
        // `class X(Protocol[T])` and `class X(Generic[T])` carry the base in
        // the subscript's value.
        let base = match base {
            Expr::Subscript(subscript) => subscript.value.as_ref(),
            other => other,
        };
        let Some(resolved) = bindings.resolve(base) else {
            continue;
        };
        if config.is_deterministic_marker(&resolved.qualified) {
            return Blessing::Marked {
                class: class.name.as_str().to_owned(),
            };
        }
        let base_grants = config.grants_for_protocol(&resolved.qualified);
        if config.is_shim_protocol(&resolved.qualified) {
            protocol.get_or_insert_with(|| resolved.qualified.clone());
            grants.extend(base_grants.into_iter().map(str::to_owned));
        }
    }
    match protocol {
        Some(protocol) => Blessing::Shim { protocol, grants },
        None => Blessing::None,
    }
}

struct Scanner<'a> {
    bindings: &'a Bindings,
    config: &'a Resolved,
    policy: Policy,
    blessing: Blessing,
    findings: Vec<Finding>,
}

impl<'a> Scanner<'a> {
    fn push(&mut self, kind: FindingKind, range: TextRange) {
        self.findings.push(Finding { kind, range });
    }

    /// Records a sink named at `range`, unless the blessing covers it.
    fn sink(&mut self, qualified: &str, usage: Usage, range: TextRange) -> bool {
        let Some(hit) = self.config.sinks().match_name(qualified, usage) else {
            return false;
        };
        let allowed = matches!(self.policy, Policy::Prod) && self.blessing.covers(&hit.capability);
        if !allowed {
            let context = self.blessing.context();
            self.push(
                FindingKind::Sink {
                    capability: hit.capability,
                    name: qualified.to_owned(),
                    matched: hit.kind,
                    context,
                },
                range,
            );
        }
        true
    }

    fn hash_order(&mut self, construct: &'static str, range: TextRange) {
        if self.policy != Policy::Sim {
            return;
        }
        if self.blessing.covers("hash_order") {
            return;
        }
        let context = self.blessing.context();
        self.push(FindingKind::HashOrder { construct, context }, range);
    }

    /// Reports dynamic escapes at a call site. Returns true when the call was
    /// fully handled as a hole.
    fn check_hole_call(&mut self, call: &ruff_python_ast::ExprCall) -> bool {
        let Some(resolved) = self.bindings.resolve(&call.func) else {
            return false;
        };
        let name = resolved.qualified;
        if self.config.is_opaque_allowed(&name) {
            return false;
        }
        if name == "__import__" || name == "importlib" || name.starts_with("importlib.") {
            self.push(
                FindingKind::Hole(Hole::DynamicImport { name }),
                call.range(),
            );
            return true;
        }
        if name == "eval" || name == "exec" {
            self.push(FindingKind::Hole(Hole::DynamicEval { name }), call.range());
            return true;
        }
        if matches!(name.as_str(), "getattr" | "setattr" | "delattr") {
            let target = call.arguments.args.first();
            let attribute = call.arguments.args.get(1);
            let literal_attribute = matches!(attribute, Some(Expr::StringLiteral(_)));
            if !literal_attribute
                && let Some(target) = target
                && let Some(target) = self.bindings.resolve(target)
                && target.kind == NameKind::Module
            {
                self.push(
                    FindingKind::Hole(Hole::DynamicAttribute {
                        function: name,
                        module: target.qualified,
                    }),
                    call.range(),
                );
                return true;
            }
        }
        false
    }

    /// Assignment to an attribute of an imported module replaces behaviour the
    /// analysis has already judged.
    fn check_monkeypatch(&mut self, target: &Expr) {
        let Expr::Attribute(attribute) = target else {
            return;
        };
        let Some(base) = self.bindings.resolve(&attribute.value) else {
            return;
        };
        if base.kind != NameKind::Module {
            return;
        }
        let qualified = format!("{}.{}", base.qualified, attribute.attr.as_str());
        if self.config.is_opaque_allowed(&qualified) {
            return;
        }
        self.push(
            FindingKind::Hole(Hole::Monkeypatch { target: qualified }),
            attribute.range(),
        );
    }

    fn check_import_fence(&mut self, target: &str, range: TextRange) {
        if self.policy != Policy::Sim {
            return;
        }
        let Some(hit) = self.config.sinks().match_module(target) else {
            return;
        };
        let SinkKind::ModuleFence(module) = hit.kind else {
            return;
        };
        self.push(
            FindingKind::ImportFence {
                capability: hit.capability,
                module,
            },
            range,
        );
    }
}

impl<'a> Visitor<'a> for Scanner<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ClassDef(class) => {
                for decorator in &class.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(arguments) = class.arguments.as_ref() {
                    // Bases are evaluated in the enclosing scope, so they are
                    // scanned before the class blessing takes effect.
                    self.visit_arguments(arguments);
                }
                let outer = std::mem::replace(
                    &mut self.blessing,
                    blessing_of(class, self.bindings, self.config),
                );
                self.visit_body(&class.body);
                self.blessing = outer;
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    self.check_import_fence(alias.name.as_str(), stmt.range());
                }
            }
            Stmt::ImportFrom(import) => {
                let base = if import.level == 0 {
                    import.module.as_ref().map(|m| m.as_str().to_owned())
                } else {
                    None
                };
                if let Some(base) = base {
                    self.check_import_fence(&base, stmt.range());
                    for alias in &import.names {
                        if alias.name.as_str() != "*" {
                            let qualified = format!("{base}.{}", alias.name.as_str());
                            self.check_import_fence(&qualified, stmt.range());
                        }
                    }
                }
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    self.check_monkeypatch(target);
                }
                visitor::walk_stmt(self, stmt);
            }
            Stmt::AugAssign(aug) => {
                self.check_monkeypatch(&aug.target);
                visitor::walk_stmt(self, stmt);
            }
            Stmt::AnnAssign(ann) => {
                self.check_monkeypatch(&ann.target);
                visitor::walk_stmt(self, stmt);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => {
                if self.check_hole_call(call) {
                    self.visit_arguments(&call.arguments);
                    return;
                }
                if let Some(resolved) = self.bindings.resolve(&call.func) {
                    let arguments = call.arguments.args.len() + call.arguments.keywords.len();
                    if matches!(resolved.qualified.as_str(), "set" | "frozenset") {
                        self.hash_order("set() call", call.range());
                    }
                    // A resolved callee is matched as a whole; its dotted
                    // prefix is not a separate naming of anything.
                    self.sink(
                        &resolved.qualified,
                        Usage::Called { arguments },
                        call.range(),
                    );
                } else {
                    self.visit_expr(&call.func);
                }
                self.visit_arguments(&call.arguments);
            }
            Expr::Set(set) => {
                self.hash_order("set literal", set.range());
                visitor::walk_expr(self, expr);
            }
            Expr::SetComp(comp) => {
                self.hash_order("set comprehension", comp.range());
                visitor::walk_expr(self, expr);
            }
            Expr::Attribute(attribute) => {
                if matches!(attribute.ctx, ExprContext::Store | ExprContext::Del) {
                    // Handled by the monkeypatch check; the write is not a use.
                    visitor::walk_expr(self, &attribute.value);
                    return;
                }
                match self.bindings.resolve(expr) {
                    // A resolved dotted chain is matched as a whole: its
                    // prefixes are not separate namings.
                    Some(resolved) => {
                        self.sink(&resolved.qualified, Usage::Referenced, attribute.range());
                    }
                    None => visitor::walk_expr(self, expr),
                }
            }
            Expr::Name(name) => {
                if matches!(name.ctx, ExprContext::Store | ExprContext::Del) {
                    return;
                }
                if let Some(resolved) = self.bindings.resolve(expr) {
                    self.sink(&resolved.qualified, Usage::Referenced, name.range());
                }
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}
