# trigpoint-core — the spec check (stopgap, v0)

Status: **implemented** as `crates/trigpoint-core` + `trigp spec check`.
This is deliberately a stopgap: a validator for hand-written invariant files
so the invariant/evidence workflow can be exercised on a real codebase
before the capability taxonomy and the code-adjacent declaration format
exist. What it validates — the ID scheme, the evidence-kind vocabulary, the
review model — is what carries over; the file format is expected to become
a generated artifact once declarations live beside the tests.

## Scope

An invariant is a claim about the system. Each one records which **kinds of
evidence it owes** (`requires`), optionally **where that evidence is**
(`[evidence]`), and optionally **who has assessed that evidence**
(`[review]`). The check reports, per invariant:

- which required kinds have no evidence at all (no `[evidence]` table) or
  are missing from it;
- which pointers name nothing that exists — a test the test runner does not
  list, an unknown lint provider, a file that is not there;
- which evidence nobody has reviewed, and which commit-pinned reviews have
  gone stale because the reviewed sources changed since.

Findings carry a severity. **Errors** mean the spec is wrong (unresolvable
pointers, reviews of undeclared evidence, unknown commits, structural
problems with the files) and always fail the run. **Gaps** mean the spec is
honest but incomplete (missing or unreviewed evidence, stale reviews) and
fail only under `--strict`. **Info** is context (nothing required, extra
evidence beyond `requires`, staleness that could not be assessed).

Two design rules from the discussion that produced this:

- `requires` is the contract and stays even when `[evidence]` is absent, so
  the first thing a new invariant tells you is what you owe.
- A review is an assessment of an *evidential step* — "this test actually
  constitutes evidence for this claim" — separate from the evidence
  existing or passing. That is the failure mode of agent-written tests:
  green, confident, and not testing the invariant. Reviews are per evidence
  kind and may be pinned to a commit so the check can defend them.

## Non-scope

- Decisions, assumptions, and the capability taxonomy: `derived_from` is a
  free string until decisions are first-class records.
- Running tests or judging pass/fail: the check verifies evidence *exists*,
  not that it is green. Run status is per-run data and belongs elsewhere.
- Multiplicity ("at least two property tests"): a list of pointers is
  accepted; counts are not enforced.
- Python test pointers: resolution is cargo-only today. The pointer scheme
  (`<target>::<path>`) is the runner's own naming, so pytest node ids slot in
  as a second resolver without changing the format.
- Per-function staleness: reviews are checked against the test *target's*
  sources (a crate's `src/`, or an integration-test file and its module
  directory), not the individual test body.

## File format: `spec/invariants/<id>.toml`

One invariant per file; the file stem must equal `id`. The schema is strict:
unknown fields and unknown evidence kinds are parse errors.

```toml
[invariant]
id = "kv.put.last-writer-wins"          # stable dotted name = file stem
statement = """
Two puts to the same key leave the later one observable by get.
"""
kind = "domain"                         # system | domain
property = "metamorphic"                # validity | postcondition | metamorphic
                                        # | inductive | model-based (domain only)
requires = ["property", "dst"]          # evidence kinds owed
rationale = "..."                       # optional
derived_from = "decision.consistency"   # optional, free-form for now

[evidence]                              # optional; kind -> pointer or [pointers]
property = "kv::tests::put_then_get_returns_latest"
dst = ["kv::tests::torn_write", "kv::tests::partition"]

[review]                                # optional; kind -> who has assessed it
property = { agent = true, human = "9b84f15" }   # bool, or a commit to pin
dst = { agent = false, human = false }
```

Evidence kinds and how their pointers resolve:

| kind | pointer | resolved against |
|---|---|---|
| `example`, `property`, `dst` | `<target>::<module::path::test>` | `cargo test -- --list` output; target is the crate name (underscores) for unit tests, the file stem for integration tests |
| `lint` | provider name | `triglint`, `trigpoint-pylint` |
| `model`, `proof` | path relative to the spec directory | the file exists |

A review mark is `false` (unreviewed), `true` (reviewed, unpinned), or a
commit hash (reviewed as of that revision). Missing `[review]` entries mean
unreviewed by both roles.

## Data/control flow

1. **Load** (`spec/load.rs`): every `*.toml` directly in the spec dir is
   parsed as an `InvariantFile`. Unreadable/unparseable files, stem≠id, and
   duplicate ids are collected as `SpecProblem`s (always errors) without
   stopping the rest.
2. **Resolve** (`resolve.rs`): unless `--no-resolve`, `cargo test
   --workspace --no-run --message-format=json` builds the test binaries;
   each is run with `--list` and every `name: test` line becomes
   `<target>::<name>` in a `TestSet`, which also records each target's
   source paths for staleness. Target names are normalized `-`→`_`.
3. **Check** (`check.rs`, pure): per invariant — nothing required → info;
   no `[evidence]` → `NoEvidence` listing `requires`; otherwise
   `MissingEvidence` per absent required kind and `ExtraEvidence` per
   unrequired present kind; every pointer resolved by its kind's scheme
   (`UnresolvedPointer` on failure); `ReviewWithoutEvidence` for review
   keys with no evidence; then for every evidence kind and each role,
   `Unreviewed`, or for a pinned commit: `UnknownCommit` if git does not
   have it, `StaleReview` if the evidence's sources changed since,
   `StalenessUnknown` if there is no oracle or no known sources.
4. **Staleness** (`git.rs`): `ChangeOracle` trait; `GitOracle` shells out
   to `git cat-file -e <commit>^{commit}` and `git diff --quiet <commit> --
   <paths>` (exit 1 = changed). Test pointers map to paths through the
   `TestSet`'s target records; file pointers are their own paths; provider
   pointers have no sources and are reported as unassessable.
5. **Report** (`report.rs`): text rendering grouped per invariant with a
   severity column and a summary line; exit code from
   `has_errors() || (strict && has_gaps())`.

## CLI

```
trigp spec check [-C <dir>] [--spec-dir spec/invariants] [--strict] [--no-resolve]
```

`--no-resolve` skips the cargo build (fast spec-only linting; test pointers
are then taken on faith and pinned reviews of tests report as unassessable).

## Related files

| file | role |
|---|---|
| `crates/trigpoint-core/src/spec/mod.rs` | schema: `InvariantFile`, `Invariant`, `EvidenceKind` (closed vocabulary + pointer scheme), `Evidence`, `Review`, `ReviewMark` |
| `crates/trigpoint-core/src/spec/load.rs` | directory loading, `SpecProblem`, `Loaded` |
| `crates/trigpoint-core/src/resolve.rs` | `TestIndex`/`TestSet`, `cargo::collect` (build + `--list`) |
| `crates/trigpoint-core/src/git.rs` | `ChangeOracle` trait, `GitOracle` |
| `crates/trigpoint-core/src/check.rs` | `check`, `Finding`, `Severity`, `Report`, `CheckContext`, `LINT_PROVIDERS` |
| `crates/trigpoint-core/src/report.rs` | text rendering |
| `crates/trigpoint-cli/src/spec.rs` | `trigp spec check` arguments, orchestration, exit code |
| `examples/sim-demo/spec/invariants/` | seed spec: one clean invariant, one with a missing kind, one unreviewed |

## Invariants and constraints

- The evidence-kind vocabulary is closed and validated at parse time in
  `requires`, `[evidence]`, and `[review]`: a typo can never mean "nothing
  required" or "no review".
- One invariant per file and stem = id, so a file *is* an invariant and two
  agents cannot create the same one without a reported duplicate.
- Pointer resolution never parses sources: test pointers are matched
  against the runner's own listing, so they are exact.
- The check is pure over (`Loaded`, `TestSet`, `ChangeOracle`); the cargo
  and git integrations are behind traits/modules and the checker is
  unit-tested with fakes.
- Severity is decided by the finding, not the caller; the CLI only chooses
  whether gaps fail the run.
- Nothing about run status (pass/fail, mutation scores) lives in the spec.
