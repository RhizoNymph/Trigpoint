# Trigpoint Overview

```yaml
Overview:
  description: >
    Trigpoint is a tool for spec-driven development with agents. It matches
    system invariants to tests and requires evidence that code conforms to its
    spec. The first deliverable is triglint: a dylint-based linter that
    enforces the deterministic-simulation-testing (DST) contract — all
    nondeterminism must flow through declared shim traits, and simulation
    builds must be fully deterministic. Motivation and the research
    direction beyond the linter are recorded in docs/design.md.
  subsystems:
    triglint: >
      Dylint lint library (nightly-pinned, rustc_private). Sim mode:
      whole-program MIR call-graph reachability from configured simulation
      roots, reporting reachable nondeterminism sinks (call sinks and
      structural type sinks like RandomState). Prod mode: per-crate check
      that nondeterminism is only introduced inside declared shim-trait
      impls (shim_nondeterminism). Lives in triglint/ as its own workspace
      because it links rustc internals; analyzed codebases stay on stable.
      Its config schema lives in trigpoint-config, reached by path dep.
    trigpoint-config: >
      Stable crate holding the triglint.toml schema (serde), discovery, and
      the builtin sink database — deliberately rustc-free so every consumer
      can share it. Carries a lenient passthrough for the [python] table
      until the Python analysis lands its typed schema here.
    trigpoint-shims: >
      Stable, dependency-free crate exporting marker traits (DeterministicShim)
      that analyzed codebases use to declare simulation shim impls.
    trigpoint-core: >
      Future home of the spec/invariant/evidence bookkeeping engine.
      Placeholder today.
    trigpoint-pylint: >
      Stable-workspace library enforcing the same DST contract on Python
      sources, in Rust on ruff's parser (ruff_python_parser/ruff_python_ast
      pinned = 0.0.13). It has no MIR and no types, so it works on names:
      per-module binding tables resolve expressions to qualified names, which
      are matched against a Python sink database. Prod mode
      (shim-nondeterminism) allows a sink to be named — called or merely
      referenced — only inside a class whose bases include a declared shim
      protocol granting the capability. Sim mode (sim-nondeterminism) walks
      the import graph from configured root modules and allows zero sinks in
      the closure, with the import chain as the witness. Dynamic access
      (importlib, eval/exec, non-literal getattr on modules, monkeypatching,
      opaque imports) is reported as unresolved warnings rather than assumed
      safe. Diagnostics render through annotate-snippets.
    trigpoint-cli: >
      Orchestrator binary (`trigp`, package name `trigpoint`). `trigp lint`
      detects which targets a workspace declares: dylint metadata in a
      Cargo.toml runs triglint via cargo-dylint (DYLINT_RUSTFLAGS=-Zalways-
      encode-mir merged in, --fresh cache busting), a [python] section in
      triglint.toml runs trigpoint-pylint in-process; --rust/--python
      restrict. A deny-level finding from either fails the run. Spec database
      and evidence aggregation are future work.
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

    A Python codebase declares the same intent in the same file: a [python]
    section of triglint.toml names source_roots, [[python.shims]] protocol
    classes with their grants, and [python.sim] root modules. `trigp lint`
    reads it without a compiler — trigpoint-pylint parses the sources with
    ruff's parser, builds a module table and per-module binding tables,
    resolves expressions to qualified names, and runs the same two rules:
    prod mode scans every module for sinks named outside a granting shim
    class, sim mode walks the import closure from the roots and allows none
    at all. Anything it cannot resolve is reported as an unresolved warning,
    so what the analysis did not see is stated rather than assumed.

Features Index:
  triglint_sim_mode:
    description: >
      Whole-program analysis asserting zero nondeterminism sinks (call and
      type sinks) reachable from declared simulation roots, including
      indirect targets collected at vtable coercions, function-pointer
      casts, and callable provenance inside constants.
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
      CLI target detection and orchestration: cargo-dylint for the Rust
      target (env merging, cache busting) when a Cargo.toml declares dylint
      libraries, trigpoint-pylint in-process for the Python target when
      triglint.toml has a [python] section, both when both, --rust/--python
      to restrict. Exit codes are unified across the two.
    entry_points:
      - crates/trigpoint-cli/src/lint.rs
      - crates/trigpoint-cli/src/lint/dylint.rs
      - crates/trigpoint-cli/src/lint/python.rs
    depends_on: [triglint_sim_mode]
    doc: docs/features/triglint.md
  trigpoint_shims_markers:
    description: >
      DeterministicShim marker trait consumed by triglint to attribute
      violations to impls that claim determinism.
    entry_points: [crates/trigpoint-shims/src/lib.rs]
    depends_on: []
    doc: docs/features/triglint.md
  python_linter:
    description: >
      Enforces the DST shim contract on Python codebases, in Rust on ruff's
      parser (pinned = 0.0.13). Prod mode (shim-nondeterminism): sinks may
      only be named — called or referenced — inside classes whose bases
      include a declared shim protocol granting the capability; a
      deterministic marker among the bases annuls the grants. Sim mode
      (sim-nondeterminism): the import-graph closure from configured root
      modules must name zero sinks, with the import chain as the witness;
      marked implementations that do are reported as broken claims. Dynamic
      access (importlib, eval/exec, non-literal getattr on modules,
      monkeypatching, star imports and opaque imports) is reported as
      unresolved warnings. Escape hatch: `# triglint: allow(<lint>)` on the
      line, the line above, or the enclosing def/class header. The `[python]`
      config schema lives in the crate for now, slated to move into the
      shared trigpoint-config crate (which accepts the section as an
      unvalidated passthrough today).
    entry_points:
      - crates/trigpoint-pylint/src/lib.rs
      - crates/trigpoint-pylint/src/prodcheck.rs
      - crates/trigpoint-pylint/src/simscope.rs
      - crates/trigpoint-cli/src/lint/python.rs
      - triglint.toml
    depends_on: [trigp_lint]
    doc: docs/features/python-linter.md
```
