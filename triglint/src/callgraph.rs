//! Monomorphized MIR call-graph traversal from simulation roots.
//!
//! The worklist walks `ty::Instance`s (def + concrete generic args), so
//! generic calls resolve precisely and the visited set is finite. Every call
//! edge is classified, in order: sink match → violation; body available →
//! traverse; otherwise trusted/allow-listed → skip, foreign → ffi violation,
//! anything else → unresolved hole in the guarantee.

use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::DefId;
use rustc_middle::mir::interpret::{AllocId, GlobalAlloc, Scalar};
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    self, Body, CastKind, ConstValue, Location, Rvalue, Statement, StatementKind, TerminatorKind,
};
use rustc_middle::ty::adjustment::PointerCoercion;
use rustc_middle::ty::{
    self, Instance, InstanceKind, Ty, TyCtxt, TypeVisitableExt, TypingEnv, VtblEntry,
};
use rustc_span::Span;

use crate::config::Resolved;

/// How control reaches a frame from its parent. Indirect edges are recorded
/// at the point the indirection is *created*, not where it is invoked: a
/// vtable's methods are collected at the unsizing coercion, a function's at
/// the cast that reifies it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A direct call (or drop) terminator.
    Call,
    /// A method of a vtable built by an unsizing coercion in the parent.
    Vtable,
    /// A function or closure reified into a pointer in the parent.
    FnPtr,
}

/// One node in the traversal tree. `call_span` is the span of the edge in
/// the parent's body (`None` for roots); `parent_local` records whether that
/// span belongs to the local crate and is therefore safe to attach
/// diagnostics to.
struct Frame<'tcx> {
    instance: Instance<'tcx>,
    parent: Option<usize>,
    call_span: Option<Span>,
    parent_local: bool,
    edge_kind: EdgeKind,
}

/// A step of a root-to-sink witness chain, rendered for diagnostics.
/// `call_span`/`edge_kind` describe the *outgoing* edge to the next step.
pub struct ChainStep {
    pub def_path: String,
    pub call_span: Option<Span>,
    pub is_local: bool,
    pub edge_kind: EdgeKind,
}

/// What kind of sink a violation matched.
pub enum SinkKind {
    /// A call edge to a sink function (or unaudited foreign item).
    Call,
    /// A sink *type* appearing in a reachable instance's generic arguments
    /// (e.g. RandomState as a hash map's hasher parameter). Detected
    /// structurally because MIR inlining inside std erases the constructor
    /// call edges.
    Type,
}

pub struct Violation {
    pub kind: SinkKind,
    pub capability: String,
    pub sink_path: String,
    /// Root first; the sink itself is `sink_path`, not a chain step.
    pub chain: Vec<ChainStep>,
    /// Deepest call span on the chain that lies in local code.
    pub primary_span: Span,
    /// Def-path of a type on the chain that implements a deterministic
    /// marker trait — a broken determinism claim.
    pub broken_claim: Option<String>,
}

pub enum UnresolvedKind {
    MissingMir { def_path: String },
    InlineAsm,
}

pub struct Unresolved {
    pub kind: UnresolvedKind,
    pub primary_span: Span,
    pub root: String,
}

pub struct Analysis<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    config: &'a Resolved,
    typing_env: TypingEnv<'tcx>,
    /// ADTs implementing a configured deterministic marker trait.
    marked_types: FxHashSet<DefId>,
    frames: Vec<Frame<'tcx>>,
    visited: FxHashMap<Instance<'tcx>, usize>,
    worklist: Vec<usize>,
    reported_violations: FxHashSet<(usize, String)>,
    reported_unresolved: FxHashSet<Span>,
    visited_allocs: FxHashSet<AllocId>,
    pub violations: Vec<Violation>,
    pub unresolved: Vec<Unresolved>,
}

impl<'a, 'tcx> Analysis<'a, 'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>, config: &'a Resolved) -> Self {
        Self {
            tcx,
            config,
            typing_env: TypingEnv::fully_monomorphized(),
            marked_types: collect_marked_types(tcx, config),
            frames: Vec::new(),
            visited: FxHashMap::default(),
            worklist: Vec::new(),
            reported_violations: FxHashSet::default(),
            reported_unresolved: FxHashSet::default(),
            visited_allocs: FxHashSet::default(),
            violations: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    pub fn run(&mut self, roots: &[Instance<'tcx>]) {
        for &root in roots {
            self.enqueue(root, None, None, false, EdgeKind::Call);
        }
        while let Some(idx) = self.worklist.pop() {
            self.visit_frame(idx);
        }
    }

    fn enqueue(
        &mut self,
        instance: Instance<'tcx>,
        parent: Option<usize>,
        call_span: Option<Span>,
        parent_local: bool,
        edge_kind: EdgeKind,
    ) {
        if self.visited.contains_key(&instance) {
            return;
        }
        let idx = self.frames.len();
        self.frames.push(Frame {
            instance,
            parent,
            call_span,
            parent_local,
            edge_kind,
        });
        self.visited.insert(instance, idx);
        self.worklist.push(idx);
    }

    fn visit_frame(&mut self, idx: usize) {
        let instance = self.frames[idx].instance;
        let body = self.tcx.instance_mir(instance.def);
        let caller_local = instance.def_id().is_local();
        // Unresolved-edge warnings are suppressed inside trusted crates:
        // std/core dispatch through dyn (fmt, panic hooks) constantly, and
        // the user can't act on those. Sink matching is unaffected.
        let caller_trusted = self
            .config
            .is_trusted_crate(&self.crate_name_of(instance.def_id()));
        let mut constants = ConstOperands::default();
        constants.visit_body(body);
        for constant in constants.0 {
            self.collect_const_operand(idx, caller_local, caller_trusted, instance, constant);
        }
        for block in body.basic_blocks.iter() {
            for statement in &block.statements {
                self.visit_statement(idx, caller_local, caller_trusted, instance, body, statement);
            }
            let terminator = block.terminator();
            let span = terminator.source_info.span;
            match &terminator.kind {
                TerminatorKind::Call { func, .. } | TerminatorKind::TailCall { func, .. } => {
                    let func_ty = self.monomorphize(instance, func.ty(body, self.tcx));
                    match func_ty.kind() {
                        ty::FnDef(def_id, args) => {
                            self.visit_call(idx, caller_local, caller_trusted, *def_id, args, span);
                        }
                        // Indirect: the pointee was collected where it was
                        // reified (see `visit_statement`).
                        ty::FnPtr(..) => {}
                        _ => {}
                    }
                }
                TerminatorKind::Drop { place, .. } => {
                    let place_ty = self.monomorphize(instance, place.ty(body, self.tcx).ty);
                    let drop_instance = Instance::resolve_drop_glue(self.tcx, place_ty);
                    self.handle_callee(
                        idx,
                        caller_local,
                        caller_trusted,
                        drop_instance,
                        span,
                        EdgeKind::Call,
                    );
                }
                TerminatorKind::InlineAsm { .. } => {
                    if !caller_trusted {
                        self.push_unresolved(idx, UnresolvedKind::InlineAsm, span, caller_local);
                    }
                }
                _ => {}
            }
        }
    }

    /// Collects the *creation* of indirection. A virtual call can only select
    /// a vtable that some reachable unsizing coercion built, and an indirect
    /// call can only reach a function that some reachable cast reified, so
    /// enqueueing targets here covers call sites the traversal cannot resolve.
    /// Over-approximate by construction: a collected vtable method counts as
    /// reachable even if it is never actually invoked.
    fn visit_statement(
        &mut self,
        idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        instance: Instance<'tcx>,
        body: &Body<'tcx>,
        statement: &Statement<'tcx>,
    ) {
        // `Assign` is the only statement kind carrying an `Rvalue`.
        let StatementKind::Assign(assign) = &statement.kind else {
            return;
        };
        let Rvalue::Cast(CastKind::PointerCoercion(coercion, _), operand, target_ty) = &assign.1
        else {
            return;
        };
        let span = statement.source_info.span;
        let operand_ty = self.monomorphize(instance, operand.ty(body, self.tcx));
        match coercion {
            PointerCoercion::Unsize => {
                let target_ty = self.monomorphize(instance, *target_ty);
                self.collect_vtable(idx, caller_local, caller_trusted, operand_ty, target_ty, span);
            }
            PointerCoercion::ReifyFnPointer(_) => {
                if let ty::FnDef(def_id, args) = *operand_ty.kind()
                    && let Some(callee) =
                        Instance::resolve_for_fn_ptr(self.tcx, self.typing_env, def_id, args)
                {
                    self.handle_callee(
                        idx,
                        caller_local,
                        caller_trusted,
                        callee,
                        span,
                        EdgeKind::FnPtr,
                    );
                }
            }
            PointerCoercion::ClosureFnPointer(_) => {
                if let ty::Closure(def_id, args) = *operand_ty.kind() {
                    let callee =
                        Instance::resolve_closure(self.tcx, def_id, args, ty::ClosureKind::FnOnce);
                    self.handle_callee(
                        idx,
                        caller_local,
                        caller_trusted,
                        callee,
                        span,
                        EdgeKind::FnPtr,
                    );
                }
            }
            _ => {}
        }
    }

    /// Enqueues everything an unsizing coercion to `dyn Trait` makes
    /// dynamically callable: the vtable's methods and the object's drop glue.
    fn collect_vtable(
        &mut self,
        idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        source_ty: Ty<'tcx>,
        target_ty: Ty<'tcx>,
        span: Span,
    ) {
        // The queries below assume monomorphic input; a lint must degrade
        // rather than ICE on MIR shapes it did not anticipate.
        if source_ty.has_param() || target_ty.has_param() {
            return;
        }
        let Some((source_ty, target_ty)) = self.unsizing_tails(source_ty, target_ty) else {
            return;
        };
        // Other unsizings (array to slice) build no vtable, and a dyn-to-dyn
        // upcast reuses one that was built at the original coercion.
        let ty::Dynamic(predicates, ..) = target_ty.kind() else {
            return;
        };
        if source_ty.is_trait() {
            return;
        }

        self.collect_dyn_target(
            idx,
            caller_local,
            caller_trusted,
            source_ty,
            predicates,
            span,
        );
    }

    /// Enqueues everything callable through a vtable for `source_ty` viewed
    /// as the object type described by `predicates`.
    fn collect_dyn_target(
        &mut self,
        idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        source_ty: Ty<'tcx>,
        predicates: &'tcx ty::List<ty::PolyExistentialPredicate<'tcx>>,
        span: Span,
    ) {
        if let Some(principal) = predicates.principal() {
            let trait_ref = self
                .tcx
                .instantiate_bound_regions_with_erased(principal.with_self_ty(self.tcx, source_ty));
            if !trait_ref.has_escaping_bound_vars() && !trait_ref.has_non_region_param() {
                for entry in self.tcx.vtable_entries(trait_ref) {
                    // Supertrait pointers (`TraitVPtr`) need no separate
                    // visit: their methods are already listed here.
                    if let VtblEntry::Method(callee) = entry {
                        self.handle_callee(
                            idx,
                            caller_local,
                            caller_trusted,
                            *callee,
                            span,
                            EdgeKind::Vtable,
                        );
                    }
                }
            }
        }

        if source_ty.needs_drop(self.tcx, self.typing_env) {
            let callee = Instance::resolve_drop_glue(self.tcx, source_ty);
            self.handle_callee(
                idx,
                caller_local,
                caller_trusted,
                callee,
                span,
                EdgeKind::Vtable,
            );
        }
    }

    /// Constants and statics can carry function pointers and vtables in
    /// their bytes (`static TABLE: &[fn()]`, `const HANDLER: &dyn Trait`),
    /// which no cast in any body reifies. Evaluating the constant and
    /// following its provenance recovers those targets.
    fn collect_const_operand(
        &mut self,
        idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        instance: Instance<'tcx>,
        constant: mir::ConstOperand<'tcx>,
    ) {
        let span = constant.span;
        let konst = self.monomorphize(instance, constant.const_);
        let Ok(value) = konst.eval(self.tcx, self.typing_env, span) else {
            return;
        };
        let alloc_id = match value {
            ConstValue::Scalar(Scalar::Ptr(pointer, _)) => pointer.provenance.alloc_id(),
            ConstValue::Indirect { alloc_id, .. } | ConstValue::Slice { alloc_id, .. } => alloc_id,
            _ => return,
        };
        self.collect_alloc(idx, caller_local, caller_trusted, alloc_id, span);
    }

    fn collect_alloc(
        &mut self,
        idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        alloc_id: AllocId,
        span: Span,
    ) {
        // Statics may reference each other cyclically.
        if !self.visited_allocs.insert(alloc_id) {
            return;
        }
        let Some(alloc) = self.tcx.try_get_global_alloc(alloc_id) else {
            return;
        };
        match alloc {
            GlobalAlloc::Function { instance, .. } => {
                self.handle_callee(
                    idx,
                    caller_local,
                    caller_trusted,
                    instance,
                    span,
                    EdgeKind::FnPtr,
                );
            }
            GlobalAlloc::VTable(ty, predicates) => {
                self.collect_dyn_target(idx, caller_local, caller_trusted, ty, predicates, span);
            }
            GlobalAlloc::Memory(alloc) => {
                let pointers: Vec<AllocId> = alloc
                    .inner()
                    .provenance()
                    .ptrs()
                    .values()
                    .map(|provenance| provenance.alloc_id())
                    .collect();
                for pointer in pointers {
                    self.collect_alloc(idx, caller_local, caller_trusted, pointer, span);
                }
            }
            GlobalAlloc::Static(def_id) => {
                if let Ok(alloc) = self.tcx.eval_static_initializer(def_id) {
                    let pointers: Vec<AllocId> = alloc
                        .inner()
                        .provenance()
                        .ptrs()
                        .values()
                        .map(|provenance| provenance.alloc_id())
                        .collect();
                    for pointer in pointers {
                        self.collect_alloc(idx, caller_local, caller_trusted, pointer, span);
                    }
                }
            }
            GlobalAlloc::TypeId { .. } => {}
        }
    }

    /// Peels a coercion down to the pair of types actually being unsized,
    /// mirroring rustc's `find_tails_for_unsizing` but returning `None`
    /// instead of ICEing on shapes it does not recognise.
    fn unsizing_tails(&self, source: Ty<'tcx>, target: Ty<'tcx>) -> Option<(Ty<'tcx>, Ty<'tcx>)> {
        match (source.kind(), target.kind()) {
            (ty::Pat(source, _), ty::Pat(target, _)) => self.unsizing_tails(*source, *target),
            (ty::Ref(_, source, _), ty::Ref(_, target, _) | ty::RawPtr(target, _))
            | (ty::RawPtr(source, _), ty::RawPtr(target, _)) => Some(
                self.tcx
                    .struct_lockstep_tails_for_codegen(*source, *target, self.typing_env),
            ),
            _ => {
                if let (Some(source), Some(target)) = (source.boxed_ty(), target.boxed_ty()) {
                    return Some(self.tcx.struct_lockstep_tails_for_codegen(
                        source,
                        target,
                        self.typing_env,
                    ));
                }
                // A wrapper implementing `CoerceUnsized` (Rc, Arc, Pin, ...):
                // recurse through the single field whose type is coerced.
                // rustc reads that field's index off `CustomCoerceUnsized`,
                // which lives in the monomorphization crate; the field whose
                // type differs identifies it just as well.
                let (ty::Adt(source_def, source_args), ty::Adt(target_def, target_args)) =
                    (source.kind(), target.kind())
                else {
                    return None;
                };
                if source_def != target_def || !source_def.is_struct() {
                    return None;
                }
                for field in &source_def.non_enum_variant().fields {
                    let source_field = self
                        .tcx
                        .normalize_erasing_regions(self.typing_env, field.ty(self.tcx, source_args));
                    let target_field = self
                        .tcx
                        .normalize_erasing_regions(self.typing_env, field.ty(self.tcx, target_args));
                    if source_field != target_field {
                        return self.unsizing_tails(source_field, target_field);
                    }
                }
                None
            }
        }
    }

    fn monomorphize<T>(&self, instance: Instance<'tcx>, value: T) -> T
    where
        T: ty::TypeFoldable<TyCtxt<'tcx>>,
    {
        instance.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            self.typing_env,
            ty::EarlyBinder::bind(value),
        )
    }

    fn visit_call(
        &mut self,
        caller_idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        def_id: DefId,
        args: ty::GenericArgsRef<'tcx>,
        span: Span,
    ) {
        match Instance::try_resolve(self.tcx, self.typing_env, def_id, args) {
            Ok(Some(callee)) => {
                self.handle_callee(
                    caller_idx,
                    caller_local,
                    caller_trusted,
                    callee,
                    span,
                    EdgeKind::Call,
                );
            }
            Ok(None) => {
                if !caller_trusted {
                    let def_path = self.tcx.def_path_str(def_id);
                    self.push_unresolved(
                        caller_idx,
                        UnresolvedKind::MissingMir { def_path },
                        span,
                        caller_local,
                    );
                }
            }
            Err(_) => {}
        }
    }

    fn handle_callee(
        &mut self,
        caller_idx: usize,
        caller_local: bool,
        caller_trusted: bool,
        callee: Instance<'tcx>,
        span: Span,
        edge_kind: EdgeKind,
    ) {
        let def_id = callee.def_id();
        let paths = self.render_paths(def_id);
        let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let crate_name = self.crate_name_of(def_id);

        // 1. Sink match happens first so fencing/trusting a crate can never
        //    mask a declared sink inside it.
        if let Some(capability) = self.config.match_sink(&crate_name, &path_refs) {
            let capability = capability.to_owned();
            self.push_violation(
                caller_idx,
                caller_local,
                SinkKind::Call,
                &capability,
                &paths[0],
                span,
            );
            return;
        }

        // 1b. Type sinks: a sink type anywhere in the callee's generic
        //     arguments (HashMap's hasher parameter reaches every insert/
        //     get/iter instance). Checked on the edge, before trust, so
        //     inlined-away constructors inside std cannot hide it. The
        //     callee is still traversed afterwards.
        if let Some((type_path, capability)) = self.match_type_sink_in_args(callee.args) {
            self.push_violation(
                caller_idx,
                caller_local,
                SinkKind::Type,
                &capability,
                &type_path,
                span,
            );
        }

        match callee.def {
            // The call itself is indirect, but every vtable it could select
            // was built by an unsizing coercion in reachable code, and those
            // sites enqueue the impl's methods. Nothing to do here.
            InstanceKind::Virtual(..) => return,
            InstanceKind::Intrinsic(..) => return,
            _ => {}
        }

        // 2. Traverse whenever a body exists, regardless of crate: generic
        //    code in trusted crates (iterator adapters, sort_by, ...) can
        //    call back into user closures, so trust must never fence off
        //    traversal — only excuse missing bodies.
        if self.has_body(callee) {
            self.enqueue(callee, Some(caller_idx), Some(span), caller_local, edge_kind);
            return;
        }

        // 3. Opaque callee: trusted or explicitly allowed → silent. A
        //    trusted caller extends trust to its opaque callees (std's
        //    internal libc/syscall shims).
        let trusted = self.config.is_trusted_crate(&crate_name)
            || caller_trusted
            || self.config.is_opaque_allowed(&path_refs);
        if trusted {
            return;
        }
        if self.tcx.is_foreign_item(def_id) {
            self.push_violation(
                caller_idx,
                caller_local,
                SinkKind::Call,
                "ffi",
                &paths[0],
                span,
            );
        } else {
            self.push_unresolved(
                caller_idx,
                UnresolvedKind::MissingMir {
                    def_path: paths[0].clone(),
                },
                span,
                caller_local,
            );
        }
    }

    fn has_body(&self, instance: Instance<'tcx>) -> bool {
        match instance.def {
            InstanceKind::Item(def_id) => self.tcx.is_mir_available(def_id),
            InstanceKind::Intrinsic(..) | InstanceKind::Virtual(..) => false,
            _ => true, // compiler-generated shims always have synthetic MIR
        }
    }

    fn render_paths(&self, def_id: DefId) -> Vec<String> {
        render_def_paths(self.tcx, def_id)
    }

    fn crate_name_of(&self, def_id: DefId) -> String {
        self.tcx.crate_name(def_id.krate).to_string()
    }

    /// Scans an instance's generic arguments (deeply) for configured sink
    /// types. Returns the matched type's def-path and capability.
    fn match_type_sink_in_args(
        &self,
        args: ty::GenericArgsRef<'tcx>,
    ) -> Option<(String, String)> {
        for arg in args {
            let Some(ty) = arg.as_type() else { continue };
            for nested in ty.walk() {
                let Some(nested_ty) = nested.as_type() else { continue };
                if let ty::Adt(adt, _) = nested_ty.kind() {
                    let paths = self.render_paths(adt.did());
                    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                    if let Some(capability) = self.config.match_type_sink(&path_refs) {
                        return Some((paths[0].clone(), capability.to_owned()));
                    }
                }
            }
        }
        None
    }

    fn push_violation(
        &mut self,
        caller_idx: usize,
        caller_local: bool,
        kind: SinkKind,
        capability: &str,
        sink_path: &str,
        span: Span,
    ) {
        let root_idx = self.root_of(caller_idx);
        if !self
            .reported_violations
            .insert((root_idx, sink_path.to_owned()))
        {
            return;
        }
        let chain = self.chain_to(caller_idx, Some(span), caller_local);
        let primary_span = self.deepest_local_span(&chain, caller_idx);
        let broken_claim = self.broken_claim_on(caller_idx);
        self.violations.push(Violation {
            kind,
            capability: capability.to_owned(),
            sink_path: sink_path.to_owned(),
            chain,
            primary_span,
            broken_claim,
        });
    }

    fn push_unresolved(
        &mut self,
        caller_idx: usize,
        kind: UnresolvedKind,
        span: Span,
        caller_local: bool,
    ) {
        if !self.reported_unresolved.insert(span) {
            return;
        }
        let chain = self.chain_to(caller_idx, Some(span), caller_local);
        let primary_span = self.deepest_local_span(&chain, caller_idx);
        let root_idx = self.root_of(caller_idx);
        let root = self
            .tcx
            .def_path_str(self.frames[root_idx].instance.def_id());
        self.unresolved.push(Unresolved {
            kind,
            primary_span,
            root,
        });
    }

    fn root_of(&self, mut idx: usize) -> usize {
        while let Some(parent) = self.frames[idx].parent {
            idx = parent;
        }
        idx
    }

    /// Chain from the root down to (and including) the frame at `idx`; the
    /// final edge span (into the sink / unresolved callee) is passed in.
    fn chain_to(
        &self,
        idx: usize,
        final_span: Option<Span>,
        final_span_local: bool,
    ) -> Vec<ChainStep> {
        let mut steps = Vec::new();
        let mut cursor = Some(idx);
        // The edge out of the frame being processed: its span (kept only
        // when local, so diagnostics never point into a dependency) and how
        // control travels along it.
        let mut edge = (
            final_span.filter(|_| final_span_local),
            final_span_local,
            EdgeKind::Call,
        );
        while let Some(i) = cursor {
            let frame = &self.frames[i];
            steps.push(ChainStep {
                def_path: self.tcx.def_path_str(frame.instance.def_id()),
                call_span: edge.0,
                is_local: edge.1,
                edge_kind: edge.2,
            });
            edge = (
                frame.call_span.filter(|_| frame.parent_local),
                frame.parent_local,
                frame.edge_kind,
            );
            cursor = frame.parent;
        }
        steps.reverse();
        steps
    }

    /// The most specific local span to anchor the diagnostic to: the deepest
    /// local call edge on the chain, falling back to the root's def span
    /// (roots are always local).
    fn deepest_local_span(&self, chain: &[ChainStep], caller_idx: usize) -> Span {
        chain
            .iter()
            .rev()
            .find_map(|step| step.call_span.filter(|_| step.is_local))
            .unwrap_or_else(|| {
                let root = self.root_of(caller_idx);
                self.tcx.def_span(self.frames[root].instance.def_id())
            })
    }

    /// Walks the chain looking for a frame belonging to an impl whose self
    /// type implements a deterministic marker trait.
    fn broken_claim_on(&self, mut idx: usize) -> Option<String> {
        loop {
            let def_id = self.frames[idx].instance.def_id();
            if let Some(adt_did) = self.impl_self_adt(def_id)
                && self.marked_types.contains(&adt_did)
            {
                return Some(self.tcx.def_path_str(adt_did));
            }
            match self.frames[idx].parent {
                Some(parent) => idx = parent,
                None => return None,
            }
        }
    }

    fn impl_self_adt(&self, def_id: DefId) -> Option<DefId> {
        let parent = self.tcx.parent(def_id);
        if !matches!(self.tcx.def_kind(parent), rustc_hir::def::DefKind::Impl { .. }) {
            return None;
        }
        self.tcx
            .type_of(parent)
            .instantiate_identity()
            .skip_normalization()
            .ty_adt_def()
            .map(|adt| adt.did())
    }
}

/// Gathers every constant a body mentions, in statements and terminators
/// alike, so their bytes can be scanned for callable provenance.
#[derive(Default)]
struct ConstOperands<'tcx>(Vec<mir::ConstOperand<'tcx>>);

impl<'tcx> Visitor<'tcx> for ConstOperands<'tcx> {
    fn visit_const_operand(&mut self, constant: &mir::ConstOperand<'tcx>, _location: Location) {
        self.0.push(*constant);
    }
}

/// Renders every def-path spelling matching should consider: the plain
/// `def_path_str` and, for local defs (printed without a crate prefix),
/// the crate-qualified form.
pub fn render_def_paths(tcx: TyCtxt<'_>, def_id: DefId) -> Vec<String> {
    let plain = tcx.def_path_str(def_id);
    if def_id.is_local() && !plain.starts_with('<') {
        let qualified = format!("{}::{}", tcx.crate_name(def_id.krate), plain);
        vec![plain, qualified]
    } else {
        vec![plain]
    }
}

/// All ADTs implementing any configured deterministic marker trait, across
/// every crate in the graph.
pub fn collect_marked_types<'tcx>(tcx: TyCtxt<'tcx>, config: &Resolved) -> FxHashSet<DefId> {
    let mut marked = FxHashSet::default();
    for trait_def_id in tcx.all_traits_including_private() {
        let plain = tcx.def_path_str(trait_def_id);
        let qualified = format!("{}::{}", tcx.crate_name(trait_def_id.krate), plain);
        if !config.matches_marker(&[plain.as_str(), qualified.as_str()]) {
            continue;
        }
        for impl_def_id in tcx.all_impls(trait_def_id) {
            let trait_ref = tcx.impl_trait_ref(impl_def_id);
            if let Some(adt) = trait_ref
                .instantiate_identity()
                .skip_normalization()
                .self_ty()
                .ty_adt_def()
            {
                marked.insert(adt.did());
            }
        }
    }
    marked
}

/// Resolves the configured sim roots among this crate's bodies. Generic
/// roots are a config error reported by the caller.
pub fn resolve_roots<'tcx>(
    tcx: TyCtxt<'tcx>,
    config: &Resolved,
) -> (Vec<Instance<'tcx>>, Vec<(DefId, String)>) {
    let mut roots = Vec::new();
    let mut generic_roots = Vec::new();
    let crate_name = tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).to_string();
    for def_id in tcx.hir_body_owners() {
        if !matches!(
            tcx.def_kind(def_id),
            rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
        ) {
            continue;
        }
        let plain = tcx.def_path_str(def_id.to_def_id());
        let qualified = format!("{crate_name}::{plain}");
        if !config.matches_root(&[plain.as_str(), qualified.as_str()]) {
            continue;
        }
        if tcx
            .generics_of(def_id.to_def_id())
            .requires_monomorphization(tcx)
        {
            generic_roots.push((def_id.to_def_id(), plain));
            continue;
        }
        roots.push(Instance::mono(tcx, def_id.to_def_id()));
    }
    (roots, generic_roots)
}
