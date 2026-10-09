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

| Invariant | Created | Removed | Added to | Deleted from |
| --- | --- | --- | --- | --- |
| clock.monotonic | — | — | evidence.property = "clock::never_rewinds" | — |
| storage.durable | ✓ | — | invariant.kind = "system"<br>invariant.statement = "Acknowledged writes survive restart." | — |

Each changed invariant has one row, ordered by ID:

- **Created / Removed:** whether the invariant exists only in the head / base
  snapshot. An ID rename is a removal plus a creation.
- **Added to / Deleted from:** field/value facts added or removed, including
  statement, rationale, kind, property, requirements, evidence, and reviews.
  A replacement shows its old value under deleted and its new value under
  added. Created and removed rows show all their corresponding facts.

Comments, TOML formatting, array ordering, duplicate list values, and the
single-pointer versus list shorthand do not affect the diff. Omitted review
marks and explicit `false` both mean unreviewed. Unchanged invariants are
omitted. Spec text is escaped for Markdown, including HTML and mentions.

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
**1** means invalid spec structure in either snapshot, a Git/API error, or
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
invariants. For another repository, install a trusted version of `trigp` and
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
      - name: Report invariant changes
        run: trigp spec diff
      - name: Update PR comment
        if: github.event.pull_request.head.repo.full_name == github.repository && github.actor != 'dependabot[bot]'
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
