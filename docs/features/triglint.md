# triglint — determinism lints (v1.1)

## Scope

Two complementary guarantees:

**Sim mode** (`sim_nondeterminism` deny + `sim_unresolved` warn):

> From the declared simulation entry points ("roots"), no nondeterminism
> sink is reachable through the monomorphized call graph.

In a simulation build every shim-typed generic parameter is instantiated with
a sim impl (e.g. `T: ClockShim` = `SimClock`), so the check needs no
capability-granting logic: the reachable set must simply contain zero sinks.
Where the analysis cannot see (dyn dispatch, function pointers, missing MIR
in untrusted crates), it emits a separate *unresolved* warning so the
guarantee is stated honestly rather than silently weakened.

Sinks come in two kinds: **call sinks** (functions/foreign items matched by
def-path, prefix, or whole-crate fence) and **type sinks** — ADTs whose mere
presence in a reachable instance's generic arguments carries a capability.
Type sinks exist because std's shipped MIR is post-inlining: the call edge to
`RandomState::new` is inlined away inside `HashMap::new`, but the hasher type
parameter reaches every map operation's instance structurally.

**Prod mode** (`shim_nondeterminism` deny, enabled by declaring `[[shims]]`):

> Nondeterminism may only be *introduced* inside an impl of a shim trait
> whose grants cover the capability.

Introduction points are always direct — the call site of a sink, or a sink
type written into a body — so this check is per-crate and syntactic: every
local body is scanned, looking through closures to the enclosing item.
Consuming shims from prod code (`clock.now_millis()` on `C: ClockShim`) is
naturally sanctioned because generic trait calls never match the sink
database. Impls for `DeterministicShim`-marked types receive **no** grants:
a sim impl must not touch anything. Escape hatch:
`#[allow(shim_nondeterminism)]` on the item (emission is node-level, so
lint-level attributes work).

The two modes answer different questions: prod mode "where does
nondeterminism enter my code?", sim mode "can any of it reach the simulation?"
Third-party dependency internals are prod-mode out-of-scope (you can't
refactor them into your shims); sim mode polices them by reachability.

## Non-scope (deferred)

- **Polymorphic capability summaries** (reachability-based prod analysis
  that excuses private helpers only called from blessed impls). The shipped
  prod mode is stricter: helpers must be inside the impl or explicitly
  allowed.
- **Dynamic dispatch resolution**: `dyn Trait` calls are reported as
  unresolved, not traversed.
- **Function-pointer tracking**: same treatment as dyn.
- **Per-crate summary caching**: cross-crate MIR is re-traversed each run.
- Ambient nondeterminism invisible to any call graph (thread scheduling,
  allocator/pointer-address ordering). Residual risk; covered operationally
  by single-threaded sim executors and double-run comparison, not by triglint.

## Toolchain model

- `triglint/` is its **own workspace**, pinned to a specific nightly via
  its `rust-toolchain` file (components: `rustc-dev`, `llvm-tools-preview`),
  because the lint dylib links `rustc_private`.
- Analyzed codebases **stay on stable**. `cargo dylint` builds a driver for
  the pinned nightly and runs `cargo check` under it with
  `RUSTC_WORKSPACE_WRAPPER`; the target dir is separate from the user's.
- Cross-crate traversal requires MIR for dependency bodies: run with
  `DYLINT_RUSTFLAGS="-Zalways-encode-mir"` (legal because the whole check
  runs under the driver's nightly; plain `RUSTFLAGS` must NOT be used — it
  leaks into cargo-dylint's stable-rustc probes and breaks library
  discovery). Without the flag, calls into dependencies surface as
  *unresolved* warnings instead of being traversed — degraded but honest.
  Caveat: `DYLINT_RUSTFLAGS` changes don't invalidate cargo's fingerprints;
  touch sources or clean `target/dylint` when toggling it.
- **`trigp lint` encodes all of the above**: it merges the flag into any
  existing `DYLINT_RUSTFLAGS`, never touches `RUSTFLAGS`, and `--fresh`
  clears only the analysis cache (`target/dylint/target`), keeping built
  lint libraries. Prefer it over invoking cargo-dylint by hand.

## Configuration: `triglint.toml`

Discovered by walking up from the linted crate's `CARGO_MANIFEST_DIR`; a
`TRIGLINT_CONFIG=/path/to/triglint.toml` env var overrides discovery (used by
UI tests). Missing config ⇒ the lint is inert (no roots, nothing to check).

```toml
# NB: top-level keys must precede any [table] header (TOML).
builtin_sinks = true   # merge the builtin sink database (default true)

[sim]
# Fully-qualified def-paths of non-generic functions. The whole-program
# analysis runs when the crate defining a root is compiled.
roots = ["sim_harness::main"]

[markers]
# Marker traits declaring "this impl claims determinism". Marked impls get
# no prod-mode grants, and a sim-mode sink reached through one is called out
# as a broken claim.
deterministic = ["trigpoint_shims::DeterministicShim"]

# Shim declarations. Presence of any entry enables prod mode
# (shim_nondeterminism) in every workspace crate.
[[shims]]
trait = "demo_lib::ClockShim"           # def-path, matched ± crate name
grants = ["time"]                       # capabilities its impls may touch

[prod]
type_sinks = false                      # also flag sink types outside shim
                                        # impls (off: shared prod code often
                                        # uses default hash maps on purpose;
                                        # sim mode flags them when reachable)

# Additional sinks, merged with builtins.
[[sinks]]
capability = "time"                     # free-form label, shown in diagnostics
paths = ["chrono::Utc::now"]            # exact def-path match
prefixes = []                           # def-path prefix match
crates = ["chrono"]                     # whole-crate fence: any call into it
types = []                              # ADT def-paths matched structurally

[opaque]
# Crates whose bodies we trust without MIR: no traversal, no warning.
# Sinks still match on the call edge (that is the std fence model).
trusted_crates = []                     # extends builtin: core, alloc, std,
                                        # panic_unwind, panic_abort,
                                        # compiler_builtins, hashbrown,
                                        # trigpoint_shims
allow = []                              # def-paths permitted to be opaque
```

## Builtin sink database

Defined at the std/libc boundary so std's own MIR is never needed:

| capability | sinks |
|---|---|
| time | `std::time::Instant::now`, `std::time::SystemTime::now` |
| random | **type sink** `RandomState` (all path spellings std has used) — the load-bearing HashMap detection, since std's inlined MIR erases the constructor edges; plus `RandomState::new` paths for direct calls; crates `getrandom`, `rand`, `rand_core`, `rand_chacha`, `fastrand` |
| thread | `std::thread::spawn`, `std::thread::sleep`, `std::thread::yield_now`, `std::thread::park`, `std::thread::park_timeout`, `std::thread::Builder::spawn` |
| fs | prefix `std::fs` |
| net | prefix `std::net` |
| env | prefix `std::env` |
| process | prefix `std::process` |
| io | `std::io::stdin` |
| ffi | implicit: any foreign (`extern`) item outside trusted crates and not `opaque.allow`-listed |

Stdout/stderr are deliberately not sinks (sim logging must work).

## Analysis: data/control flow

1. **Lint registration** (`triglint/src/lib.rs`): a `LateLintPass` whose
   `check_crate` runs once per crate with full `TyCtxt`.
2. **Config load**: resolve `triglint.toml` (env override → walk-up). Parse
   with serde; merge builtin sinks unless disabled.
2b. **Prod mode** (`prodcheck.rs`, when `[[shims]]` is non-empty): for every
   local `Fn`/`AssocFn`/`Closure` body, walk closure parents to the enclosing
   item and compute its blessing (shim-trait impl → grants; marked self type
   → none; default methods in the shim trait itself → grants). Scan the
   body's call terminators: resolve concrete dispatch where possible
   (`TypingEnv::post_analysis`; unresolved generic calls fall back to the
   raw trait-method def, which matches crate fences but not inherent std
   paths). Sink hit not covered by the blessing ⇒ `shim_nondeterminism`,
   emitted node-level so `#[allow]` works. Runs in every workspace crate,
   independent of sim roots.
3. **Root resolution**: iterate local body owners; a body is a root if its
   def-path (with and without leading crate name) matches a configured root.
   No roots in this crate ⇒ sim analysis does nothing (it runs exactly once
   per sim binary, in the crate that defines the root).
4. **Worklist traversal** over `ty::Instance`s starting from
   `Instance::mono(root)`:
   - Fetch MIR via `tcx.instance_mir`; iterate `TerminatorKind::Call` /
     `TailCall`; monomorphize the callee type with the instance's args;
     for `FnDef`, resolve with `Instance::try_resolve`.
   - Each resolved callee is checked in order:
     1. **call-sink match** (path / prefix / crate) → violation;
     2. **type-sink match** on the callee's monomorphized generic arguments
        (deep walk) → violation; traversal still continues;
     3. **MIR available** → push to worklist (visited-set on `Instance`);
        trust never fences off traversal — generic code in trusted crates
        (iterator adapters, `sort_by`) calls back into user closures;
     4. otherwise (opaque): trusted callee crate, trusted *caller* crate, or
        allow-listed → skip; foreign item → ffi violation; else →
        *unresolved: no MIR* warning.
   - `InstanceKind::Virtual` (dyn) and `FnPtr` callee types → *unresolved*
     warning at the call site. Intrinsics and drop glue without MIR are
     skipped. Unresolved warnings (all kinds) are suppressed when the
     *calling* frame is in a trusted crate: std/core dispatch through dyn
     constantly (fmt, panic hooks) and the user cannot act on those; sink
     matching is unaffected by this suppression.
   - A parent map `Instance → (caller Instance, call span)` records the
     traversal tree for witness reconstruction.
5. **Diagnostics**:
   - `SIM_NONDETERMINISM` (**deny** by default): primary span is the deepest
     call site on the witness chain that lies in local code; notes list the
     full root → … → sink chain with capability label. If any frame on the
     chain belongs to an impl whose self type implements a configured
     deterministic marker trait, the diagnostic says which determinism claim
     is broken.
   - `SIM_UNRESOLVED` (**warn** by default): dyn calls, fn-pointer calls,
     missing-MIR edges — each a hole in the guarantee.
   - Each (sink instance, root) pair is reported once (first witness found).

## Related files

| file | role |
|---|---|
| `triglint/Cargo.toml`, `triglint/rust-toolchain` | nightly-pinned dylint library workspace |
| `triglint/src/lib.rs` | lint registration, `check_crate` orchestration |
| `triglint/src/config.rs` | `triglint.toml` schema (serde), discovery, builtin sink DB, unit tests |
| `triglint/src/callgraph.rs` | sim mode: monomorphized worklist traversal, call/type sink matching, witness chains |
| `triglint/src/prodcheck.rs` | prod mode: per-body direct-sink scan, blessing resolution |
| `triglint/src/diagnostics.rs` | violation/unresolved emission (sim: crate-level; prod: node-level for `#[allow]`) |
| `triglint/ui/*` | sim-mode UI fixtures + expected stderr (no shims configured) |
| `triglint/ui_prod/*` | prod-mode UI fixtures (ClockShim grants time) |
| `crates/trigpoint-shims/src/lib.rs` | `DeterministicShim` marker trait (stable, no deps) |
| `crates/trigpoint-cli/src/lint.rs` | `trigp lint`: cargo-dylint orchestration (DYLINT_RUSTFLAGS merge, `--fresh` cache bust, exit-code propagation) |
| `examples/sim-demo/` | end-to-end target: ClockShim, SimClock (marked), SystemClock (blessed prod impl), allowed hidden sink, sim root, `triglint.toml` |

## Invariants and constraints

- The lint dylib and driver share one pinned nightly; analyzed code never
  requires nightly in its own toolchain file.
- Roots must be non-generic fns (`Instance::mono` requires it); config
  violations of this are reported as errors, not silently skipped.
- The traversal must terminate: visited-set keyed on the full `Instance`
  (def + generic args), which is finite for a monomorphized program.
- Sink matching happens **before** traversal/trust checks, so fencing a crate
  as trusted can never mask a declared sink inside it.
- No violation and no unresolved warning ⇒ every reachable call edge from the
  roots was resolved and sink-free; this is the evidence statement sim mode
  contributes to the broader trigpoint bookkeeping.
- Type sinks are matched on generic arguments at every call edge, before
  trust checks, so std's MIR inlining and trusted-crate fencing can never
  hide a nondeterministic type.
- Prod-mode blessing is lexical (enclosing item through closures) and
  capability-specific; marked-deterministic self types annul all grants.
- Prod diagnostics are emitted at the body's HIR node so
  `#[allow(shim_nondeterminism)]` works; sim diagnostics are whole-program
  and not item-allowable by design.
- `trigpoint-shims` must remain dependency-free and stable so adding the
  marker costs analyzed projects nothing.
