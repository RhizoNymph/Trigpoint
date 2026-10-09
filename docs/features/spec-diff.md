# Invariant spec diffs

`trigp spec diff` compares committed invariant specs at the merge base of a
base branch and a head branch, then prints a GitHub Markdown table. It does
not check out branches, modify files, resolve evidence, or run project tests.

```sh
trigp spec diff --base origin/main
trigp spec diff --base release --head my-feature
trigp spec diff -C examples/sim-demo --base origin/main
trigp spec diff --base origin/main --fail-on-change
```

The table has one row per changed invariant and these columns:

| Column | Contents |
| --- | --- |
| Invariant | Current ID (old ID for removals) |
| Change | Created, Removed, Modified, or Renamed |
| ID / Filename | Old and new identity/path, including detected renames |
| Statement / Kind / Property / Requires / Rationale / Derived from | Separate columns for each structured invariant field |
| Evidence | Added/deleted pointers, labeled by evidence kind |
| Test body | Changes to referenced test functions, including signatures and non-doc attributes |
| Test docstring | Changes to the function's Rust doc comments / doc attributes |
| Agent review / Human review | Separate per-evidence-kind review status and status changes |

Each changed cell shows `−` old content and `+` new content; unchanged cells
show `—`. Multiline replacements omit common leading/trailing lines. Created
and removed invariants show all their fields. Review columns also retain
unchanged current statuses, so a test edit remains visibly unreviewed even if
its review record did not change. Spec text is escaped for Markdown, HTML,
and mentions.

Comments, TOML formatting, array ordering, duplicate list values, and the
single-pointer versus list shorthand do not affect spec field comparisons.
Unchanged invariants are omitted, but a **test-source-only edit** still produces
a row for every invariant referencing that test.

**Kind is immutable:** changing `invariant.kind` for an existing invariant is
an error, including across a detected rename. The table is printed/published
before returning failure, regardless of `--fail-on-change`.

Renames use Git's rename detection, with a fallback for uniquely matching
otherwise identical declarations. A rename appears as one row with old/new ID
and filename values. An ambiguous or extensively rewritten rename can appear
as creation/removal: without a separate immutable identity, it cannot always
be distinguished from replacing one invariant with another. Filename and ID
must still agree at both revisions.

## Test source and review scope

The command statically parses Rust source at each Git revision, using committed
Cargo manifests to map `target::module::test` pointers. It supports ordinary
library, binary, and integration-test targets, inline/external modules, and
literal `#[path]` modules. Rust doc comments and `#[doc = "..."]` attributes
are separated from function bodies. Only referenced functions participate:
unrelated functions changing in the same file do not create a diff.

No build scripts or test code are executed. Macro-generated tests, ambiguous
conditional definitions, missing functions, and non-Rust pointer formats are
reported as **unresolved**, not silently treated as verified unchanged tests.
This does not expand macros, evaluate dynamic doc attributes, or track helper
functions, fixtures, dependencies, model/proof contents, or lint implementation
changes. These are direct-source comparisons, not proof of unchanged behavior.

Reviews remain scoped to an **evidence kind**, using the existing `[review]`
records; the command does not invent field-level approvals:

- Missing/`false`: **Unreviewed**.
- `true`: **Reviewed (unpinned; freshness unknown)**. This cannot establish
  that the newly changed revision was reviewed.
- Commit pin: compare the invariant definition (including identity/path), that
  kind's pointers, and its direct test source/doc comments against the pinned
  revision. A mismatch is **Stale**. A matching ancestor pin with resolvable test
  source is **Reviewed** for that scope.
- Missing commits, unresolved sources, non-ancestor pins, and non-test evidence
  whose contents are not assessed show **Unknown**, never a fresh approval.

To record review of a test change, set the role's pin to a commit containing the
reviewed spec and test contents. Review metadata itself is excluded from the
comparison, so adding the pin in a later commit does not invalidate it. Edits to
an invariant definition invalidate its pinned evidence reviews. Unpinned
claims always retain unknown freshness.

## Revisions, paths, and exit codes

`--base` accepts a Git ref or commit. Without it, the command uses the GitHub
PR event's base SHA, then `origin/$GITHUB_BASE_REF`, then `origin/HEAD`, local
`main`, or local `master`, in that order. Git does not store the branch a
feature was created from: pass `--base` for release branches, stacked PRs,
or any other base that differs from these defaults. No remote fetch is
performed automatically.

`--head` defaults to the PR event's head SHA in GitHub Actions and `HEAD`
elsewhere. Using the PR head avoids treating GitHub's synthetic merge commit
as branch content. `--base` and `--head` override event defaults independently.
The report includes the resolved merge-base and head hashes. Multiple merge
bases are rejected rather than picking one arbitrarily.

`-C` selects the workspace directory. `--spec-dir` defaults to
`spec/invariants`, relative to that workspace; only directly contained
`*.toml` files are read, matching `spec check`. Paths must be relative and
cannot contain `..`. Missing directories are empty snapshots, allowing the
first invariant to be introduced or the last one removed. Both missing means
no changes. Uncommitted changes are excluded.

Exit status is **0** on a successful report, including when invariants change.
**1** means invalid spec structure in either snapshot, a Git/API error, an invariant kind change, or
(with `--fail-on-change`) any invariant changes. Clap argument errors use
status **2**. This command validates the spec schema and filename/ID match;
use `trigp spec check` separately to validate evidence resolution and coverage.

## Persistent GitHub PR comments

Install and authenticate the [GitHub CLI](https://cli.github.com/manual/gh_api),
then opt into publishing:

```sh
trigp spec diff --base origin/main --comment --repo owner/repo --pr 123
```

In a GitHub PR workflow, `--comment` reads the repository, PR number, base
SHA, and head SHA from `GITHUB_EVENT_PATH`; repository fallback is
`GITHUB_REPOSITORY`. Outside that context, pass `--repo` and `--pr` explicitly.
The token needs permission to write PR comments, normally
`pull-requests: write` ([GitHub comment API](https://docs.github.com/en/rest/issues/comments)).
Use `GH_TOKEN` or `gh auth login` for authentication; credentials are handled
by `gh`.

A hidden marker identifies one comment per repository-relative spec directory.
The command searches every page of PR comments and patches the marked comment,
or creates it if absent. Unrelated comments are left alone. Reruns with no
changes replace the old table with “No invariant changes.” Do not copy the
hidden marker into other comments. Use the same publishing identity on reruns
so it can edit its comment.

The full table is always printed to stdout. Publishing failures fail the check;
reports exceeding GitHub's comment size limit fail publication without
truncating the stdout report. `--fail-on-change` is evaluated after publishing.
Serialize runs for the same PR/spec directory to prevent concurrent creates or
older reports overwriting newer ones.

## GitHub Actions

This repository's [spec-diff workflow](../../.github/workflows/spec-diff.yml)
builds the CLI, tests the diff implementation, and reports changes to the demo's
invariants (including changes made only to their referenced tests). For another repository, install a trusted version of `trigp` and
change the command's `--spec-dir` (or omit it for `spec/invariants`). The key
workflow settings are:

```yaml
on:
  pull_request:

permissions:
  contents: read
  pull-requests: write

concurrency:
  group: spec-diff-${{ github.event.pull_request.number }}
  cancel-in-progress: true

jobs:
  spec-diff:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          persist-credentials: false
      # Install trigp here before running the next steps.
      # Give the install/build step id: build.
      - name: Report invariant changes
        run: trigp spec diff
      - name: Update PR comment
        if: always() && !cancelled() && steps.build.outcome == 'success' && github.event.pull_request.head.repo.full_name == github.repository && github.actor != 'dependabot[bot]'
        env:
          GH_TOKEN: ${{ github.token }}
        run: trigp spec diff --comment
```

Full history is needed for merge-base comparison; `fetch-depth: 0` is documented
by [actions/checkout](https://github.com/actions/checkout). Fork and Dependabot
PRs retain the report/check but skip the write step because their default tokens
are read-only. See [GitHub workflow permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax).
Do not switch this example to `pull_request_target` while building or executing
PR-controlled code. The diff itself only reads spec blobs and needs no code
execution from the compared revision.
