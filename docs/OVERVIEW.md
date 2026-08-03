# Trigpoint Overview

```yaml
Overview:
  description: >
    Trigpoint is a tool for spec-driven development with agents. It matches
    system invariants to tests and requires evidence that code conforms to its
    spec. The first deliverable is triglint: a dylint-based linter that
    enforces the deterministic-simulation-testing (DST) contract — all
    nondeterminism must flow through declared shim traits, and simulation
    builds must be fully deterministic.
  subsystems:
    triglint: >
      Dylint lint library (nightly-pinned, rustc_private). Sim mode:
      whole-program MIR call-graph reachability from configured simulation
      roots, reporting reachable nondeterminism sinks (call sinks and
      structural type sinks like RandomState). Prod mode: per-crate check
      that nondeterminism is only introduced inside declared shim-trait
      impls (shim_nondeterminism). Lives in triglint/ as its own workspace
      because it links rustc internals; analyzed codebases stay on stable.
    trigpoint-shims: >
      Stable, dependency-free crate exporting marker traits (DeterministicShim)
      that analyzed codebases use to declare simulation shim impls.
    trigpoint-core: >
      Future home of the spec/invariant/evidence bookkeeping engine.
      Placeholder today.
    trigpoint-cli: >
      Orchestrator binary (`trigp`, package name `trigpoint`). `trigp lint`
      drives cargo-dylint with DYLINT_RUSTFLAGS=-Zalways-encode-mir merged
      in, offers --fresh cache busting, and propagates exit codes. Spec
      database and evidence aggregation are future work.
    examples: >
      Example workspaces used as end-to-end integration targets for triglint
      (a toy sim harness with clock shims).
  data_flow: >
    A user codebase declares shim traits (e.g. ClockShim) and marks sim impls
    with trigpoint_shims::DeterministicShim. It configures triglint.toml at
    the workspace root (sim roots, [[shims]] grants, extra sinks). `trigp
    lint` (or `cargo dylint` with DYLINT_RUSTFLAGS=-Zalways-encode-mir)
    compiles the workspace under triglint's pinned nightly driver. In every
    workspace crate, prod mode scans local bodies for sinks introduced
    outside granting shim impls; in the crate containing a sim root, sim
    mode builds a monomorphized MIR call graph from the roots and matches
    call edges and generic-argument types against the sink database. Both
    emit deny-by-default diagnostics; sim violations carry the full
    root-to-sink witness chain.

Features Index:
  triglint_sim_mode:
    description: >
      Whole-program analysis asserting zero nondeterminism sinks (call and
      type sinks) reachable from declared simulation roots.
    entry_points: [triglint/src/lib.rs, triglint.toml]
    depends_on: [trigpoint_shims_markers]
    doc: docs/features/triglint.md
  triglint_prod_mode:
    description: >
      Per-crate shim_nondeterminism lint: sinks may only be introduced
      inside impls of [[shims]] traits whose grants cover the capability;
      DeterministicShim-marked impls get no grants.
    entry_points: [triglint/src/prodcheck.rs, triglint.toml]
    depends_on: [triglint_sim_mode, trigpoint_shims_markers]
    doc: docs/features/triglint.md
  trigp_lint:
    description: >
      CLI orchestration of cargo-dylint (env merging, cache busting, exit
      codes) so users need no dylint folklore.
    entry_points: [crates/trigpoint-cli/src/lint.rs]
    depends_on: [triglint_sim_mode]
    doc: docs/features/triglint.md
  trigpoint_shims_markers:
    description: >
      DeterministicShim marker trait consumed by triglint to attribute
      violations to impls that claim determinism.
    entry_points: [crates/trigpoint-shims/src/lib.rs]
    depends_on: []
    doc: docs/features/triglint.md
```
