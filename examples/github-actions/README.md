# Spec snapshot check in GitHub Actions

Copy [`spec-check.yml`](spec-check.yml) into `.github/workflows/spec-check.yml`
to enable checks on pull requests, pushes to `main`, and manual runs.
The example is ready to run in the Trigpoint repository against
`examples/sim-demo`; that demo deliberately has gaps, so **the strict check is
expected to fail** until they are filled.

For another Cargo workspace, install a pinned version of `trigp` in place of
the build step, change the executable path, and set `SPEC_WORKSPACE` to the
directory containing `Cargo.toml` and `spec/invariants`. Add
`--spec-dir <relative-path>` if needed. Configure the Rust toolchain and any
native dependencies required to build that workspace's test binaries.

The workflow checks the exact PR head or push/manual-run commit. The Actions
job summary identifies that commit and shows the full report grouped by
invariant, including when the command exits unsuccessfully. It does not
compare branches or post a PR comment. The full report is also in the step log.

## What it checks

The command is:

```sh
trigp spec check -C examples/sim-demo --strict
```

For every invariant, it checks:

- Required evidence kinds that have no declared evidence.
- Test pointers that do not resolve, file pointers that do not exist, and
  unknown lint providers.
- Evidence missing an **agent** review.
- Evidence missing a **human** review.
- Commit-pinned reviews that are stale, reference an unknown commit, or
  cannot be assessed.

`requires = ["example", "property"]` declares what an invariant owes.
`[evidence]` supplies pointers, and `[review]` records each role separately:

```toml
[evidence]
example = "demo_lib::tests::sim_clock_is_strictly_monotonic"

[review]
example = { agent = true, human = false }
```

This example reports that the example evidence has not been reviewed by a
human. If `property` is required but absent, it also reports missing property
evidence. Missing review entries and explicit `false` both mean unreviewed.
The current checker checks both roles for every declared evidence kind,
including evidence beyond `requires`.

## Expected demo report

The included demo currently reports:

```text
sim.clock.monotonic
  gap    missing property evidence
  gap    example evidence not reviewed by human

sim.deterministic
  ok     all required evidence present, resolved, and reviewed

sim.tick.reads-shim-clock
  gap    example evidence not reviewed by agent
  gap    example evidence not reviewed by human

summary: 0 error(s), 4 gap(s), 0 info; test pointers resolved against 3 test(s)
```

Paths are omitted above for readability. The workflow preserves the check's
nonzero exit status after writing the summary. Remove `--strict` to report
gaps without failing CI; structural and resolution errors still fail.

## Meaning of verification

This checks **recorded review state**, not the identity or quality of a review.
`true` is an unpinned assertion of review; a commit hash enables staleness
checking. Unlike `spec diff`'s direct-function comparison, `spec check` currently
checks pinned test reviews against their target's source paths, so an unrelated
edit in that target can mark a review stale. Unassessable staleness is reported
as informational and does not fail even in strict mode.

The resolver builds the workspace's tests and runs each binary with `--list`;
it does not execute the tests themselves or establish that they pass. Run
`cargo test` separately for that. Keep resolution enabled for the full check:
`--no-resolve` accepts test pointers without verifying them and cannot assess
their pinned reviews for staleness. Full Git history makes referenced review
commits available; it does not make an unpinned review fresh.

The workflow uses `pull_request` with read-only permissions and works without a
PR-comment token. The checkout pins the commit explicitly; see
[actions/checkout](https://github.com/actions/checkout). Reports use GitHub's
[job summary facility](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands#adding-a-job-summary).
