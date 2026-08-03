//! Rendering of analysis results as lint diagnostics.

use rustc_errors::{Diag, DiagCtxtHandle, Diagnostic, Level, MultiSpan};
use rustc_lint::{LateContext, LintContext};
use rustc_session::lint::Lint;
use rustc_span::Span;

use crate::callgraph::{SinkKind, Unresolved, UnresolvedKind, Violation};
use crate::prodcheck::{Context, ProdSinkKind, ProdViolation};
use crate::{SHIM_NONDETERMINISM, SIM_NONDETERMINISM, SIM_UNRESOLVED};

/// Adapter turning a decorate closure into the `Diagnostic` impl that
/// `emit_span_lint` expects (same shape clippy_utils uses internally).
struct DecorateFn<F: FnOnce(&mut Diag<'_, ()>)>(F);

impl<'a, F: FnOnce(&mut Diag<'_, ()>)> Diagnostic<'a, ()> for DecorateFn<F> {
    fn into_diag(self, dcx: DiagCtxtHandle<'a>, level: Level) -> Diag<'a, ()> {
        let mut diag = Diag::new(dcx, level, "");
        (self.0)(&mut diag);
        diag
    }
}

fn span_lint(
    cx: &LateContext<'_>,
    lint: &'static Lint,
    span: Span,
    message: String,
    decorate: impl FnOnce(&mut Diag<'_, ()>),
) {
    cx.emit_span_lint(
        lint,
        MultiSpan::from_span(span),
        DecorateFn(move |diag: &mut Diag<'_, ()>| {
            diag.primary_message(message);
            diag.span(MultiSpan::from_span(span));
            decorate(diag);
        }),
    );
}

pub fn emit_violation(cx: &LateContext<'_>, violation: &Violation) {
    let message = match violation.kind {
        SinkKind::Call => format!(
            "nondeterminism sink `{}` ({}) is reachable from a simulation root",
            violation.sink_path, violation.capability
        ),
        SinkKind::Type => format!(
            "nondeterministic type `{}` ({}) is reachable from a simulation root",
            violation.sink_path, violation.capability
        ),
    };
    span_lint(
        cx,
        SIM_NONDETERMINISM,
        violation.primary_span,
        message,
        |diag| {
            let mut rendered = String::from("call chain to sink:");
            for (i, step) in violation.chain.iter().enumerate() {
                rendered.push_str(&format!("\n  [{i}] {}", step.def_path));
            }
            rendered.push_str(&format!(
                "\n  [{}] {} <- sink",
                violation.chain.len(),
                violation.sink_path
            ));
            diag.note(rendered);
            if matches!(violation.kind, SinkKind::Type) {
                diag.note(
                    "this type carries per-process random state (e.g. hash-map iteration order differs between runs)",
                );
            }
            if let Some(claim) = &violation.broken_claim {
                diag.note(format!(
                    "`{claim}` implements a deterministic marker trait, but this sink is reachable through it: the determinism claim is broken"
                ));
            }
            match violation.kind {
                SinkKind::Call => diag.help(
                    "route this capability through a shim trait, or declare the callee in triglint.toml ([[sinks]] / [opaque])",
                ),
                SinkKind::Type => diag.help(
                    "use a deterministic replacement in simulation-reachable code (e.g. HashMap with a fixed BuildHasher, or BTreeMap)",
                ),
            };
        },
    );
}

pub fn emit_prod_violation(cx: &LateContext<'_>, violation: &ProdViolation) {
    let message = match violation.kind {
        ProdSinkKind::Call => format!(
            "nondeterminism sink `{}` ({}) called outside a shim impl granting `{}`",
            violation.sink_path, violation.capability, violation.capability
        ),
        ProdSinkKind::Type => format!(
            "nondeterministic type `{}` ({}) used outside a shim impl granting `{}`",
            violation.sink_path, violation.capability, violation.capability
        ),
    };
    let hir_id = cx.tcx.local_def_id_to_hir_id(violation.body);
    let span = violation.span;
    let context_note = match &violation.context {
        Context::Unblessed => {
            "not inside any shim impl; nondeterminism must enter through a [[shims]] trait so simulation builds can substitute it".to_owned()
        }
        Context::MarkedDeterministic { self_ty } => format!(
            "the enclosing impl is for `{self_ty}`, which is marked deterministic: sim impls receive no capability grants"
        ),
        Context::GrantsMissing { trait_path, grants } => format!(
            "the enclosing `{trait_path}` impl grants only [{}]",
            grants.join(", ")
        ),
    };
    cx.tcx.emit_node_span_lint(
        SHIM_NONDETERMINISM,
        hir_id,
        MultiSpan::from_span(span),
        DecorateFn(move |diag: &mut Diag<'_, ()>| {
            diag.primary_message(message);
            diag.span(MultiSpan::from_span(span));
            diag.note(context_note);
            diag.help(
                "move this into a granting shim impl, route it through a shim, or `#[allow(shim_nondeterminism)]` with justification",
            );
        }),
    );
}

pub fn emit_unresolved(cx: &LateContext<'_>, unresolved: &Unresolved) {
    let message = match &unresolved.kind {
        UnresolvedKind::DynDispatch => {
            "dyn-dispatch call cannot be resolved statically; the sim-mode determinism guarantee has a hole here".to_owned()
        }
        UnresolvedKind::FnPointer => {
            "indirect call through a function pointer cannot be resolved statically; the sim-mode determinism guarantee has a hole here".to_owned()
        }
        UnresolvedKind::MissingMir { def_path } => format!(
            "no MIR available for `{def_path}`; its body cannot be checked (run with DYLINT_RUSTFLAGS=-Zalways-encode-mir, or trust it via [opaque] in triglint.toml)"
        ),
        UnresolvedKind::InlineAsm => {
            "inline assembly is opaque to triglint; the sim-mode determinism guarantee has a hole here".to_owned()
        }
    };
    let root = unresolved.root.clone();
    span_lint(
        cx,
        SIM_UNRESOLVED,
        unresolved.primary_span,
        message,
        move |diag| {
            diag.note(format!("reachable from simulation root `{root}`"));
        },
    );
}
