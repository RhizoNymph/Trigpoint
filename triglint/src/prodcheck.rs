//! Prod mode: find where nondeterminism is *introduced* outside shim impls.
//!
//! Unlike sim mode (a whole-program reachability question), introduction
//! points are always direct: the call site of a sink, or a sink type written
//! into a body. So this check is per-crate and syntactic — every local body
//! is scanned, and a sink hit is fine only if the body sits (lexically,
//! looking through closures) inside an impl of a `[[shims]]` trait whose
//! grants cover the capability. Impls for types marked deterministic receive
//! no grants: a sim impl must not touch anything.
//!
//! Trait-generic dispatch (`clock.now_millis()` on `C: ClockShim`) never
//! matches the sink database, so consuming shims from prod code is naturally
//! sanctioned. Third-party dependency internals are out of scope here — you
//! cannot refactor them into your shims; sim mode polices them by
//! reachability.

use rustc_data_structures::fx::FxHashSet;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_middle::mir::TerminatorKind;
use rustc_middle::ty::{self, Instance, TyCtxt, TypingEnv};
use rustc_span::Span;

use crate::callgraph::{collect_marked_types, render_def_paths};
use crate::config::Resolved;

/// Why a body was not blessed for the capability it touched.
pub enum Context {
    /// Not inside any shim impl.
    Unblessed,
    /// Inside a shim impl, but the self type is marked deterministic.
    MarkedDeterministic { self_ty: String },
    /// Inside a shim impl whose grants do not cover the capability.
    GrantsMissing {
        trait_path: String,
        grants: Vec<String>,
    },
}

pub enum ProdSinkKind {
    Call,
    Type,
}

pub struct ProdViolation {
    pub kind: ProdSinkKind,
    pub capability: String,
    pub sink_path: String,
    pub span: Span,
    /// Body owner, for node-level emission so `#[allow(shim_nondeterminism)]`
    /// works at the item.
    pub body: LocalDefId,
    pub context: Context,
}

/// The blessing state of a body's enclosing (non-closure) item.
enum Blessing {
    None,
    Marked { self_ty: String },
    Shim {
        trait_path: String,
        grants: Vec<String>,
    },
}

impl Blessing {
    fn covers(&self, capability: &str) -> bool {
        match self {
            Blessing::Shim { grants, .. } => grants.iter().any(|g| g == capability),
            _ => false,
        }
    }

    fn to_context(&self) -> Context {
        match self {
            Blessing::None => Context::Unblessed,
            Blessing::Marked { self_ty } => Context::MarkedDeterministic {
                self_ty: self_ty.clone(),
            },
            Blessing::Shim { trait_path, grants } => Context::GrantsMissing {
                trait_path: trait_path.clone(),
                grants: grants.clone(),
            },
        }
    }
}

pub fn run(tcx: TyCtxt<'_>, config: &Resolved) -> Vec<ProdViolation> {
    let marked_types = collect_marked_types(tcx, config);
    let mut violations = Vec::new();
    for def_id in tcx.hir_body_owners() {
        if !matches!(
            tcx.def_kind(def_id),
            DefKind::Fn | DefKind::AssocFn | DefKind::Closure
        ) {
            continue;
        }
        let owner = non_closure_owner(tcx, def_id);
        let blessing = blessing_of(tcx, config, &marked_types, owner.to_def_id());
        check_body(tcx, config, &blessing, def_id, &mut violations);
    }
    violations
}

fn check_body(
    tcx: TyCtxt<'_>,
    config: &Resolved,
    blessing: &Blessing,
    def_id: LocalDefId,
    violations: &mut Vec<ProdViolation>,
) {
    let body = tcx.optimized_mir(def_id);
    let typing_env = TypingEnv::post_analysis(tcx, def_id);
    let mut reported_types: FxHashSet<String> = FxHashSet::default();
    for block in body.basic_blocks.iter() {
        let terminator = block.terminator();
        let span = terminator.source_info.span;
        let func = match &terminator.kind {
            TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } => func,
            _ => continue,
        };
        let ty::FnDef(callee_def, args) = func.ty(body, tcx).kind() else {
            continue;
        };
        // Resolve concrete dispatch where possible; generic trait calls
        // (`C::now_millis` with `C` a type param) stay unresolved and fall
        // back to the trait method's own def — which matches crate fences
        // (`rand::Rng::gen`) but not inherent std sink paths, exactly the
        // sanctioned-consumption behavior we want.
        let callee_def = match Instance::try_resolve(
            tcx,
            typing_env,
            *callee_def,
            tcx.erase_and_anonymize_regions(args),
        ) {
            Ok(Some(instance)) => instance.def_id(),
            _ => *callee_def,
        };
        let paths = render_def_paths(tcx, callee_def);
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let crate_name = tcx.crate_name(callee_def.krate).to_string();
        if let Some(capability) = config.match_sink(&crate_name, &path_refs)
            && !blessing.covers(capability)
        {
            violations.push(ProdViolation {
                kind: ProdSinkKind::Call,
                capability: capability.to_owned(),
                sink_path: paths[0].clone(),
                span,
                body: def_id,
                context: blessing.to_context(),
            });
        }
        if config.prod_type_sinks()
            && let Some((type_path, capability)) = type_sink_in_args(tcx, config, args)
            && !blessing.covers(&capability)
            && reported_types.insert(type_path.clone())
        {
            violations.push(ProdViolation {
                kind: ProdSinkKind::Type,
                capability,
                sink_path: type_path,
                span,
                body: def_id,
                context: blessing.to_context(),
            });
        }
    }
}

fn type_sink_in_args<'tcx>(
    tcx: TyCtxt<'tcx>,
    config: &Resolved,
    args: ty::GenericArgsRef<'tcx>,
) -> Option<(String, String)> {
    for arg in args {
        let Some(ty) = arg.as_type() else { continue };
        for nested in ty.walk() {
            let Some(nested_ty) = nested.as_type() else { continue };
            if let ty::Adt(adt, _) = nested_ty.kind() {
                let paths = render_def_paths(tcx, adt.did());
                let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                if let Some(capability) = config.match_type_sink(&path_refs) {
                    return Some((paths[0].clone(), capability.to_owned()));
                }
            }
        }
    }
    None
}

/// Walks up through closure/coroutine bodies to the enclosing item.
fn non_closure_owner(tcx: TyCtxt<'_>, def_id: LocalDefId) -> LocalDefId {
    let mut current = def_id;
    while matches!(tcx.def_kind(current), DefKind::Closure) {
        current = tcx.local_parent(current);
    }
    current
}

fn blessing_of(
    tcx: TyCtxt<'_>,
    config: &Resolved,
    marked_types: &FxHashSet<DefId>,
    item: DefId,
) -> Blessing {
    if tcx.def_kind(item) != DefKind::AssocFn {
        return Blessing::None;
    }
    let parent = tcx.parent(item);
    let trait_def = match tcx.def_kind(parent) {
        // Default method bodies in the shim trait definition itself are
        // blessed with the trait's grants.
        DefKind::Trait => parent,
        DefKind::Impl { of_trait: true } => {
            let trait_ref = tcx.impl_trait_ref(parent);
            let trait_ref = trait_ref.instantiate_identity().skip_normalization();
            let trait_def = trait_ref.def_id;
            if is_shim_trait(tcx, config, trait_def)
                && let Some(adt) = trait_ref.self_ty().ty_adt_def()
                && marked_types.contains(&adt.did())
            {
                return Blessing::Marked {
                    self_ty: tcx.def_path_str(adt.did()),
                };
            }
            trait_def
        }
        _ => return Blessing::None,
    };
    let paths = render_def_paths(tcx, trait_def);
    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let grants = config.grants_for_trait(&path_refs);
    if grants.is_empty() {
        return Blessing::None;
    }
    Blessing::Shim {
        trait_path: paths[0].clone(),
        grants: grants.into_iter().map(str::to_owned).collect(),
    }
}

fn is_shim_trait(tcx: TyCtxt<'_>, config: &Resolved, trait_def: DefId) -> bool {
    let paths = render_def_paths(tcx, trait_def);
    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    !config.grants_for_trait(&path_refs).is_empty()
}
