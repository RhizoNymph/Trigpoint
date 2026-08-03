//! triglint — dylint lints enforcing the trigpoint deterministic-simulation
//! contract. See docs/features/triglint.md at the repo root.
//!
//! v1 is sim mode only: from the `[sim] roots` declared in `triglint.toml`,
//! no nondeterminism sink may be reachable through the monomorphized call
//! graph. Edges the analysis cannot see through (dyn dispatch, function
//! pointers, missing MIR) are surfaced as `sim_unresolved` warnings so the
//! guarantee is never silently weakened.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_data_structures;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod callgraph;
mod config;
mod diagnostics;
mod prodcheck;

use rustc_lint::{LateContext, LateLintPass};

dylint_linting::dylint_library!();

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Verifies that no nondeterminism sink (time, random, threads, fs,
    /// net, env, process, unaudited FFI, ...) is reachable through the
    /// monomorphized call graph from the simulation roots declared in
    /// `triglint.toml`.
    ///
    /// ### Why is this bad?
    ///
    /// Deterministic simulation testing is only sound if the simulation
    /// binary is actually deterministic. Any nondeterminism must enter
    /// through a shim trait whose simulation impl is deterministic.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// fn sim_main() {
    ///     let started = std::time::Instant::now(); // sink reachable: error
    /// }
    /// ```
    ///
    /// Use instead a clock shim (`ClockShim`) whose simulation impl advances
    /// virtual time deterministically.
    pub SIM_NONDETERMINISM,
    Deny,
    "nondeterminism sink reachable from a simulation root"
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Reports call edges reachable from a simulation root that triglint
    /// cannot see through: dyn dispatch, function pointers, inline assembly,
    /// and bodies with no MIR available.
    ///
    /// ### Why is this bad?
    ///
    /// Each such edge is a hole in the sim-mode determinism guarantee. The
    /// analysis reports them instead of silently assuming they are safe.
    pub SIM_UNRESOLVED,
    Warn,
    "call edge from a simulation root that cannot be statically resolved or traversed"
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// With shim traits declared via `[[shims]]` in `triglint.toml`, checks
    /// every function in the crate for direct nondeterminism-sink calls (and
    /// optionally sink types) outside an impl of a shim trait whose grants
    /// cover the capability. Impls for types marked `DeterministicShim`
    /// receive no grants.
    ///
    /// ### Why is this bad?
    ///
    /// The shim architecture only works if nondeterminism *enters* the
    /// system exclusively through shim impls: that is what lets a simulation
    /// build substitute deterministic implementations. A sink called
    /// anywhere else is nondeterminism the simulation cannot control.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// fn helper() {
    ///     std::thread::sleep(d); // outside any shim impl: error
    /// }
    /// ```
    ///
    /// Use instead an impl of a declared shim trait (e.g. a `SleepShim`)
    /// granting the `thread` capability.
    pub SHIM_NONDETERMINISM,
    Deny,
    "nondeterminism introduced outside a granting shim impl"
}

rustc_session::impl_lint_pass!(Triglint => [SIM_NONDETERMINISM, SIM_UNRESOLVED, SHIM_NONDETERMINISM]);

#[derive(Default)]
struct Triglint;

#[expect(clippy::no_mangle_with_rust_abi)]
#[unsafe(no_mangle)]
pub fn register_lints(_sess: &rustc_session::Session, lint_store: &mut rustc_lint::LintStore) {
    lint_store.register_lints(&[SIM_NONDETERMINISM, SIM_UNRESOLVED, SHIM_NONDETERMINISM]);
    lint_store.register_late_pass(|_| Box::new(Triglint));
}

impl<'tcx> LateLintPass<'tcx> for Triglint {
    fn check_crate(&mut self, cx: &LateContext<'tcx>) {
        let config = match config::load() {
            Ok(Some((_, config))) => config::Resolved::new(config),
            // No triglint.toml anywhere: the lint is inert.
            Ok(None) => return,
            Err(error) => {
                cx.tcx.dcx().err(format!("triglint: {error}"));
                return;
            }
        };

        // Prod mode: per-crate, runs wherever shims are declared,
        // independent of sim roots.
        if config.prod_enabled() {
            for violation in prodcheck::run(cx.tcx, &config) {
                diagnostics::emit_prod_violation(cx, &violation);
            }
        }

        let (roots, generic_roots) = callgraph::resolve_roots(cx.tcx, &config);
        for (def_id, path) in &generic_roots {
            cx.tcx.dcx().span_err(
                cx.tcx.def_span(*def_id),
                format!("triglint: sim root `{path}` is generic; roots must be non-generic functions"),
            );
        }
        // The whole-program analysis runs once, in the crate defining roots.
        if roots.is_empty() {
            return;
        }

        let mut analysis = callgraph::Analysis::new(cx.tcx, &config);
        analysis.run(&roots);
        for violation in &analysis.violations {
            diagnostics::emit_violation(cx, violation);
        }
        for unresolved in &analysis.unresolved {
            diagnostics::emit_unresolved(cx, unresolved);
        }
    }
}

#[test]
fn ui() {
    // Both suites run sequentially in one test so the env-based config
    // switch cannot race. SAFETY: set before the harness spawns drivers,
    // which inherit the environment.
    unsafe {
        std::env::set_var(
            config::CONFIG_ENV,
            concat!(env!("CARGO_MANIFEST_DIR"), "/ui/triglint.toml"),
        );
    }
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
    unsafe {
        std::env::set_var(
            config::CONFIG_ENV,
            concat!(env!("CARGO_MANIFEST_DIR"), "/ui_prod/triglint.toml"),
        );
    }
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui_prod");
}
