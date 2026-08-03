# triglint

Dylint lints enforcing the trigpoint deterministic-simulation contract.
Design doc: [`docs/features/triglint.md`](../docs/features/triglint.md).

Three lints:

- `sim_nondeterminism` (deny): from the `[sim] roots` declared in
  `triglint.toml`, no nondeterminism sink (time, random, threads, fs, net,
  env, process, unaudited FFI) may be reachable through the monomorphized
  MIR call graph. Sinks are both calls and *types* — `RandomState` (the
  default `HashMap` hasher) is caught structurally in generic arguments,
  since std's inlined MIR erases the constructor call edges. Diagnostics
  carry the full root-to-sink witness chain and call out broken
  `DeterministicShim` claims.
- `shim_nondeterminism` (deny, enabled by declaring `[[shims]]`): per-crate,
  sinks may only be *introduced* inside an impl of a shim trait whose
  `grants` cover the capability. Impls for `DeterministicShim`-marked types
  get no grants. `#[allow(shim_nondeterminism)]` on an item is the escape
  hatch.
- `sim_unresolved` (warn): an edge sim mode cannot see through (dyn
  dispatch, function pointers, inline asm, missing MIR) — a hole in the
  guarantee, reported instead of assumed safe.

## Usage

The analyzed codebase stays on stable; only this lint crate pins a nightly.

```sh
cargo install cargo-dylint dylint-link
```

In the analyzed workspace's `Cargo.toml`:

```toml
[workspace.metadata.dylint]
libraries = [{ path = "path/to/trigpoint/triglint" }]
```

Add a `triglint.toml` next to it (see `ui/triglint.toml`,
`ui_prod/triglint.toml`, or `examples/sim-demo/triglint.toml` for the
schema), then either use the orchestrator:

```sh
trigp lint            # trigp lint --fresh after toggling MIR flags
```

or run cargo-dylint directly:

```sh
DYLINT_RUSTFLAGS="-Zalways-encode-mir" cargo dylint --all
```

`-Zalways-encode-mir` lets sim mode traverse dependency bodies. Use
`DYLINT_RUSTFLAGS`, not `RUSTFLAGS` (which breaks cargo-dylint's toolchain
probes); note it doesn't bust cargo's cache when toggled — `trigp lint
--fresh` handles that.

## Testing

```sh
cargo test
```

UI fixtures live in `ui/`. To re-bless expected output after an intentional
diagnostic change: run the tests, review the actual output compiletest saves
to `/tmp/<fixture>.stage-id.stderr`, and copy it over `ui/<fixture>.stderr`.
