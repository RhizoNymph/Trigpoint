# Trigpoint

A WIP tool for spec-driven development with agents: it matches system
invariants to tests and requires evidence that code conforms to its spec
through different categories of testing (property testing, deterministic
simulation testing, mutation testing) and formal methods.

The first shipped piece is the **deterministic simulation testing (DST)
contract**: all nondeterminism (time, randomness, I/O, threads, FFI) must
enter the system through *shim traits*, so a simulation build can substitute
deterministic implementations — and a linter proves it.

## What works today

- **`triglint`** — dylint lints over the MIR call graph:
  - `sim_nondeterminism` (deny): no nondeterminism sink is reachable from
    your declared simulation entry points, checked by whole-program
    monomorphized reachability across crate boundaries. Sinks are both
    calls (`Instant::now`, `thread::sleep`, FFI, …) and types (`HashMap`'s
    default `RandomState` hasher, caught structurally). Diagnostics carry
    the full root-to-sink witness chain.
  - `shim_nondeterminism` (deny): nondeterminism may only be *introduced*
    inside impls of your declared shim traits, checked per-crate.
  - `sim_unresolved` (warn): anything the analysis cannot see through (dyn
    dispatch, function pointers, missing MIR) is reported as a hole rather
    than assumed safe.
- **`trigpoint-shims`** — the `DeterministicShim` marker trait: mark your
  simulation shim impls; triglint holds them to a zero-nondeterminism
  standard.
- **`trigp lint`** — CLI orchestration of cargo-dylint (correct env flags,
  cache busting, exit codes) so you don't need any dylint folklore.

The analyzed codebase stays on stable Rust; only the lint library pins a
nightly toolchain internally.

## Quickstart

```sh
cargo install cargo-dylint dylint-link
```

In your workspace's `Cargo.toml`:

```toml
[workspace.metadata.dylint]
libraries = [{ path = "path/to/trigpoint/triglint" }]
```

Next to it, a `triglint.toml`:

```toml
[sim]
roots = ["my_sim_harness::main"]

[[shims]]
trait = "my_crate::ClockShim"
grants = ["time"]
```

Then:

```sh
trigp lint          # or: DYLINT_RUSTFLAGS="-Zalways-encode-mir" cargo dylint --all
```

See `examples/sim-demo/` for a complete worked example with a clock shim,
a marked sim impl, and feature-gated violations.

## Repository layout

| path | contents |
|---|---|
| `crates/trigpoint-cli` | the `trigp` binary (package name `trigpoint`) |
| `crates/trigpoint-core` | spec/invariant/evidence engine (future work) |
| `crates/trigpoint-shims` | `DeterministicShim` marker trait |
| `triglint/` | the dylint lint library (own nightly-pinned workspace) |
| `examples/sim-demo/` | end-to-end demo workspace |
| `docs/` | architecture overview and feature docs |

Design details live in [`docs/OVERVIEW.md`](docs/OVERVIEW.md) and
[`docs/features/triglint.md`](docs/features/triglint.md); usage details in
[`triglint/README.md`](triglint/README.md).
