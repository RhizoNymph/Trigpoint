# 0001 — Capabilities are one concept; the spec references the lint vocabulary, never the reverse

- **Status:** accepted (2026-11-08)
- **Id:** `decision.capability-vocabulary-direction`
- **Context:** capability taxonomy design (first step toward first-class
  decisions/assumptions/interactions in `trigpoint-core`)

## Problem

"Capability" is used in two senses in this repository:

1. **In the linters** (`triglint`, `trigpoint-pylint`, schema in
   `trigpoint-config`): a *nondeterminism grant* — `time`, `random`, `fs`,
   `net`, `thread`, `process`, `env`, `io`, `ffi`. A free-form string label on
   a sink (`[[sinks]] capability = "time"`) that a shim trait may `grant`.
   The lint checks grant ⊇ sink capability by string equality. The config is
   self-contained.
2. **In the playbook** (`brainstorming/INVARIANT_ASSURANCE_PLAYBOOK.md`): a
   *thing the system does* — retry, deduplicate, persist the ledger — the
   root of the discovery path
   `capability → decision/assumption → further decision/assumption → interaction effects`.

When capabilities become spec nodes, the two senses must either be unified or
kept as separate vocabularies, and if unified, something has to agree with
something else.

## Decision

**Unify the concept, and make the dependency point from spec to lint config.**

- The two senses are the same concept at different altitudes. Lint
  capabilities are the *effects* of leaf capability nodes. The DST contract
  is the first worked instance of the full graph:

  ```text
  capability.wall-clock-time            (leaf; effects = ["time"])
    └─motivates→ decision.nondeterminism   "route it through ClockShim"
         ├─assumes→   assumption.sim-clock-is-deterministic
         └─motivates→ invariant.sim.deterministic   requires=[lint] → triglint
  ```

- A capability node names the effect labels it owns:

  ```toml
  # spec/capabilities/wall-clock-time.toml
  [capability]
  id = "wall-clock-time"
  effects = ["time"]          # labels from triglint.toml's sink/grant vocabulary
  ```

- **The linters do not change.** Effect labels stay free-form strings in
  `triglint.toml`; `triglint` and `trigpoint-pylint` never read `spec/`.
- **The spec check verifies agreement.** `trigpoint-core` loads
  `triglint.toml` through `trigpoint-config` (already the shared stable
  crate, discovery included) and reports:
  - an `effects` label no sink or grant in the config declares →
    unresolved pointer (error, same class as a test pointer the runner does
    not list);
  - a grant the config declares that no capability node claims → coverage
    gap ("the system grants `fs` to a shim and no capability owns it");
  - optionally, `evidence.lint = "triglint"` refined to the set of effects
    that lint run attests to, derived from the config rather than asserted.

Resulting crate graph: `trigpoint-core → trigpoint-config` (new, cheap);
`triglint → trigpoint-config` (existing); `triglint ↛ spec` (never).

## Alternatives rejected

- **Lint labels become references to spec nodes**
  (`grants = ["capability.wall-clock-time"]`, or the lint validates grants
  against declared nodes). Rejected: the lint would have to locate and parse
  spec files from inside a rustc pass, `trigpoint-config` would absorb the
  spec schema, and the lint would need an "inert when no spec" mode. The
  mechanism would know about the bookkeeping.
- **Keep two separate vocabularies.** Rejected: loses the ability to say
  which effects a lint-evidenced invariant actually covers, and leaves the
  grant list with no owner in the graph.
- **Close the effect vocabulary as an enum in `trigpoint-config`.**
  Rejected for now: users add custom sinks with project-specific labels; the
  spec check already catches typos by cross-referencing the config.

## Consequences

- Grant ↔ capability consistency is enforced at `trigp spec check` time, not
  at compile time. Acceptable: a grant without a capability node is a
  bookkeeping gap, not a determinism hole.
- Lint diagnostics keep saying `capability: time`; they cannot name the
  decision they break. If that attribution is wanted later, it belongs in
  `trigp` (the orchestrator), joining structured lint output to the graph —
  not in the lint.
- `trigpoint-core` gains a `trigpoint-config` dependency and must handle a
  missing `triglint.toml` (effects then resolve against nothing: every
  `effects` label is unresolved, which is the honest report).

## Motivates

- Capability nodes (`spec/capabilities/`) with `effects` as the bridge to the
  lint vocabulary.
- Typed `motivated_by` references replacing the free-string `derived_from`
  on invariants.
- The remaining taxonomy nodes — decisions, assumptions (`if_false`),
  interactions as nodes with coverage dispositions — are open and recorded
  separately when decided.
