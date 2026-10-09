//! Git snapshot comparison and optional GitHub PR comment publication.
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Value, json};
use thiserror::Error;
use trigpoint_core::spec::diff::{self, Snapshot};
use trigpoint_core::spec::{EvidenceKind, PointerScheme, ReviewMark, source};

#[derive(clap::Args)]
pub struct DiffArgs {
    /// Workspace directory (defaults to the current directory).
    #[arg(long, short = 'C')]
    dir: Option<PathBuf>,
    /// Spec directory relative to the workspace; only direct *.toml files count.
    #[arg(long, default_value = "spec/invariants")]
    spec_dir: PathBuf,
    /// Base branch/ref. Defaults to PR event base, origin/HEAD, main, or master.
    #[arg(long)]
    base: Option<String>,
    /// Branch/ref to compare. Defaults to PR event head or HEAD (committed only).
    #[arg(long)]
    head: Option<String>,
    /// Create or update a GitHub PR comment using authenticated `gh`.
    #[arg(long)]
    comment: bool,
    /// GitHub repository OWNER/REPO (defaults to PR event or GITHUB_REPOSITORY).
    #[arg(long, requires = "comment")]
    repo: Option<String>,
    /// Pull request number (defaults to the GitHub PR event).
    #[arg(long, requires = "comment", value_parser = clap::value_parser!(u64).range(1..))]
    pr: Option<u64>,
    /// Exit with status 1 when invariants change, after printing/posting the report.
    #[arg(long)]
    fail_on_change: bool,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct DiffError(String);
impl From<std::io::Error> for DiffError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

fn command(
    dir: &Path,
    program: &str,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<String, DiffError> {
    let mut child = Command::new(program)
        .current_dir(dir)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| DiffError(format!("failed to run {program}: {e}")))?;
    if let Some(input) = input {
        let written = child.stdin.take().expect("piped stdin").write_all(input);
        if let Err(e) = written {
            let _ = child.wait();
            return Err(e.into());
        }
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(DiffError(format!(
            "{program} {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| DiffError(format!("{program} returned non-UTF-8 output: {e}")))
}
fn git(dir: &Path, args: &[&str]) -> Result<String, DiffError> {
    command(dir, "git", args, None)
}
fn resolve(dir: &Path, reference: &str) -> Result<String, DiffError> {
    git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .map(|s| s.trim().to_owned())
}

fn snapshot(root: &Path, commit: &str, spec_dir: &str) -> Result<Snapshot, DiffError> {
    let mut result = Snapshot::new();
    // Literal pathspec prevents special characters in a directory from acting as globs.
    let listing = git(
        root,
        &[
            "ls-tree",
            "-r",
            "-z",
            commit,
            "--",
            &format!(":(literal){spec_dir}"),
        ],
    )?;
    for entry in listing.split('\0').filter(|s| !s.is_empty()) {
        let (meta, path) = entry
            .split_once('\t')
            .ok_or_else(|| DiffError("invalid git tree entry".into()))?;
        let path = Path::new(path);
        if path.parent() != Some(Path::new(if spec_dir == "." { "" } else { spec_dir }))
            || path.extension().is_none_or(|e| e != "toml")
        {
            continue;
        }
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() != 3 || !matches!(fields[0], "100644" | "100755") || fields[1] != "blob" {
            return Err(DiffError(format!(
                "{}: expected a regular invariant file",
                path.display()
            )));
        }
        let text = git(root, &["cat-file", "blob", fields[2]])?;
        diff::insert(&mut result, path, &text)
            .map_err(|e| DiffError(format!("at {commit}: {e}")))?;
    }
    Ok(result)
}

fn source_index(root: &Path, commit: &str, workspace: &str) -> Result<source::Index, DiffError> {
    let listing = git(
        root,
        &[
            "ls-tree",
            "-r",
            "-z",
            commit,
            "--",
            &format!(
                ":(literal){}",
                if workspace.is_empty() { "." } else { workspace }
            ),
        ],
    )?;
    let mut files = source::Files::new();
    for entry in listing.split('\0').filter(|s| !s.is_empty()) {
        let (meta, path) = entry
            .split_once('\t')
            .ok_or_else(|| DiffError("invalid git tree entry".into()))?;
        let path = Path::new(path);
        if path.extension().is_none_or(|e| e != "rs")
            && path.file_name().is_none_or(|n| n != "Cargo.toml")
        {
            continue;
        }
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() == 3 && matches!(fields[0], "100644" | "100755") && fields[1] == "blob" {
            files.insert(
                path.to_owned(),
                git(root, &["cat-file", "blob", fields[2]])?,
            );
        }
    }
    Ok(source::Index::build(&files))
}
fn enriched_snapshot(
    root: &Path,
    commit: &str,
    spec_dir: &str,
    workspace: &str,
) -> Result<Snapshot, DiffError> {
    let mut spec = snapshot(root, commit, spec_dir)?;
    if spec.values().any(|e| {
        e.file
            .evidence
            .iter()
            .flat_map(|e| &e.0)
            .any(|(kind, _)| kind.pointer_scheme() == PointerScheme::Test)
    }) {
        source::enrich(&mut spec, &source_index(root, commit, workspace)?);
    }
    Ok(spec)
}
fn renames(root: &Path, base: &str, head: &str) -> Result<BTreeMap<PathBuf, PathBuf>, DiffError> {
    let output = git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--name-status",
            "-z",
            "--find-renames",
            base,
            head,
            "--",
        ],
    )?;
    let mut fields = output.split('\0').filter(|s| !s.is_empty());
    let mut result = BTreeMap::new();
    while let Some(status) = fields.next() {
        let old = fields
            .next()
            .ok_or_else(|| DiffError("invalid Git diff entry".into()))?;
        if status.starts_with('R') || status.starts_with('C') {
            let new = fields
                .next()
                .ok_or_else(|| DiffError("invalid Git rename entry".into()))?;
            if status.starts_with('R') {
                result.insert(old.into(), new.into());
            }
        }
    }
    Ok(result)
}

fn review_scope(entry: &diff::Entry, kind: EvidenceKind) -> BTreeSet<String> {
    let prefixes = [
        format!("evidence.{kind} = "),
        format!("test body.{kind}."),
        format!("test docstring.{kind}."),
    ];
    entry
        .facts
        .iter()
        .filter(|f| {
            f.starts_with("invariant.")
                || f.starts_with("filename = ")
                || prefixes.iter().any(|p| f.starts_with(p))
        })
        .cloned()
        .collect()
}

/// Reviews describe an evidence kind. Unpinned flags cannot establish that a
/// particular revision was reviewed; pins must match the head's reviewed scope.
fn reviews(
    root: &Path,
    revision: &str,
    spec_dir: &str,
    workspace: &str,
    spec: &mut Snapshot,
    cache: &mut BTreeMap<String, Result<Snapshot, String>>,
) {
    for entry in spec.values_mut() {
        entry
            .facts
            .retain(|f| !f.starts_with("agent review.") && !f.starts_with("human review."));
        let kinds: BTreeSet<_> = entry
            .file
            .invariant
            .requires
            .iter()
            .copied()
            .chain(entry.file.evidence.iter().flat_map(|e| e.0.keys().copied()))
            .chain(entry.file.review.iter().flat_map(|r| r.0.keys().copied()))
            .collect();
        for kind in kinds {
            let review = entry.file.review.as_ref().and_then(|r| r.0.get(&kind));
            for (role, mark) in [
                ("agent", review.map(|r| &r.agent)),
                ("human", review.map(|r| &r.human)),
            ] {
                let status = match mark {
                    None | Some(ReviewMark::Flag(false)) => "Unreviewed".to_owned(),
                    Some(ReviewMark::Flag(true)) => {
                        "Reviewed (unpinned; freshness unknown)".to_owned()
                    }
                    Some(ReviewMark::Commit(pin)) => match resolve(root, pin) {
                        Err(_) => format!("Unknown review commit {pin}"),
                        Ok(commit) => {
                            let snapshot = cache.entry(commit.clone()).or_insert_with(|| {
                                enriched_snapshot(root, &commit, spec_dir, workspace)
                                    .map_err(|e| e.to_string())
                            });
                            match snapshot {
                                Err(_) => format!("Unknown at {pin}: review snapshot unavailable"),
                                Ok(snapshot) => match snapshot.get(&entry.file.invariant.id) {
                                    None => format!("Stale at {pin}: invariant absent or renamed"),
                                    Some(old)
                                        if review_scope(old, kind) != review_scope(entry, kind) =>
                                    {
                                        format!(
                                            "Stale at {pin}: definition, pointers, or tests changed"
                                        )
                                    }
                                    Some(old)
                                        if !old.warnings.is_empty()
                                            || !entry.warnings.is_empty() =>
                                    {
                                        format!("Unknown at {pin}: test source unresolved")
                                    }
                                    Some(_) if kind.pointer_scheme() != PointerScheme::Test => {
                                        format!(
                                            "Unknown at {pin}: non-test evidence content not assessed"
                                        )
                                    }
                                    Some(_)
                                        if entry
                                            .file
                                            .evidence
                                            .as_ref()
                                            .and_then(|e| e.0.get(&kind))
                                            .is_none_or(|p| p.iter().next().is_none()) =>
                                    {
                                        format!("Unreviewed: no {kind} evidence")
                                    }
                                    Some(_)
                                        if git(
                                            root,
                                            &["merge-base", "--is-ancestor", &commit, revision],
                                        )
                                        .is_err() =>
                                    {
                                        format!(
                                            "Unknown at {pin}: not an ancestor of this revision"
                                        )
                                    }
                                    Some(_) => format!(
                                        "Reviewed at {pin} (definition and direct test source match)"
                                    ),
                                },
                            }
                        }
                    },
                };
                entry
                    .facts
                    .insert(format!("{role} review.{kind} = {status}"));
            }
        }
    }
}

fn event() -> Result<Value, DiffError> {
    match env::var_os("GITHUB_EVENT_PATH") {
        None => Ok(Value::Null),
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|e| DiffError(format!("invalid GitHub event: {e}"))),
    }
}
fn event_string(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn run(args: DiffArgs) -> Result<ExitCode, DiffError> {
    let dir = args.dir.unwrap_or(env::current_dir()?);
    if args.spec_dir.is_absolute()
        || args
            .spec_dir
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(DiffError(
            "--spec-dir must be relative to the workspace without '..'".into(),
        ));
    }
    let root = PathBuf::from(git(&dir, &["rev-parse", "--show-toplevel"])?.trim());
    let prefix = git(&dir, &["rev-parse", "--show-prefix"])?;
    let spec_path = Path::new(prefix.trim()).join(args.spec_dir);
    let spec_dir = spec_path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    let spec_dir = if spec_dir.is_empty() {
        ".".to_owned()
    } else {
        spec_dir
    };
    let event = event()?;
    let head_ref = args
        .head
        .or_else(|| event_string(&event, "/pull_request/head/sha"))
        .unwrap_or_else(|| "HEAD".into());
    let head = resolve(&root, &head_ref)?;
    let base_ref = args
        .base
        .or_else(|| event_string(&event, "/pull_request/base/sha"))
        .or_else(|| {
            env::var("GITHUB_BASE_REF")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| format!("refs/remotes/origin/{s}"))
        })
        .or_else(|| {
            [
                "refs/remotes/origin/HEAD",
                "refs/heads/main",
                "refs/heads/master",
            ]
            .into_iter()
            .find(|r| resolve(&root, r).is_ok())
            .map(str::to_owned)
        })
        .ok_or_else(|| {
            DiffError(
                "cannot determine base branch; pass --base <ref> and fetch its history".into(),
            )
        })?;
    let base = resolve(&root, &base_ref)?;
    let merge_bases = git(&root, &["merge-base", "--all", &base, &head]).map_err(|e| {
        DiffError(format!(
            "{e}; fetch full base/head history (checkout fetch-depth: 0 in CI)"
        ))
    })?;
    let bases: Vec<_> = merge_bases.lines().collect();
    if bases.len() != 1 {
        return Err(DiffError(
            "comparison requires exactly one merge base".into(),
        ));
    }
    let mut before = enriched_snapshot(&root, bases[0], &spec_dir, prefix.trim())?;
    let mut after = enriched_snapshot(&root, &head, &spec_dir, prefix.trim())?;
    let mut cache = BTreeMap::new();
    reviews(
        &root,
        bases[0],
        &spec_dir,
        prefix.trim(),
        &mut before,
        &mut cache,
    );
    reviews(
        &root,
        &head,
        &spec_dir,
        prefix.trim(),
        &mut after,
        &mut cache,
    );
    let changes = diff::compare_with_renames(&before, &after, &renames(&root, bases[0], &head)?);
    let invalid_kind = changes.iter().any(|c| c.invalid_kind);
    let mut report = format!(
        "## Invariant spec diff\n\nSpec: {}\n\nMerge base: `{}` → Head: `{head}`\n\n{}",
        diff::escape(&spec_dir),
        bases[0],
        diff::render(&changes)
    );
    let warnings: BTreeSet<_> = before
        .values()
        .chain(after.values())
        .flat_map(|e| e.warnings.iter())
        .collect();
    for warning in warnings {
        report.push_str(&format!(
            "\nUnresolved test source: {}\n",
            diff::escape(warning)
        ));
    }
    print!("{report}");
    if args.comment {
        let repo = args
            .repo
            .or_else(|| event_string(&event, "/repository/full_name"))
            .or_else(|| env::var("GITHUB_REPOSITORY").ok())
            .ok_or_else(|| {
                DiffError("--comment requires --repo OWNER/REPO or GitHub PR event context".into())
            })?;
        let pr = args
            .pr
            .or_else(|| {
                event
                    .pointer("/pull_request/number")
                    .and_then(Value::as_u64)
            })
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                DiffError("--comment requires --pr NUMBER or GitHub PR event context".into())
            })?;
        post_comment(&dir, &repo, pr, &spec_dir, &report)?;
    }
    if invalid_kind {
        eprintln!("trigp: error: invariant kind cannot change (including across a rename)");
    }
    Ok(
        if invalid_kind || (args.fail_on_change && !changes.is_empty()) {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        },
    )
}

fn marker(spec_dir: &str) -> String {
    // Hex encoding is stable, unambiguous, and cannot terminate an HTML comment.
    let key: String = spec_dir
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("<!-- trigp-spec-diff:v1:{key} -->")
}
fn existing_comment(pages: &Value, marker: &str) -> Result<Option<u64>, DiffError> {
    let pages = pages
        .as_array()
        .ok_or_else(|| DiffError("invalid GitHub comment pages".into()))?;
    for page in pages {
        let comments = page
            .as_array()
            .ok_or_else(|| DiffError("invalid GitHub comment page".into()))?;
        for comment in comments {
            if comment["body"]
                .as_str()
                .is_some_and(|b| b.lines().next() == Some(marker))
            {
                return comment["id"]
                    .as_u64()
                    .map(Some)
                    .ok_or_else(|| DiffError("invalid GitHub comment ID".into()));
            }
        }
    }
    Ok(None)
}
fn post_comment(
    dir: &Path,
    repo: &str,
    pr: u64,
    spec_dir: &str,
    report: &str,
) -> Result<(), DiffError> {
    if repo.split('/').count() != 2
        || repo.split('/').any(|s| {
            s.is_empty()
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        return Err(DiffError("--repo must be OWNER/REPO".into()));
    }
    let marker = marker(spec_dir);
    let body = format!("{marker}\n{report}");
    if body.chars().count() > 65_536 {
        return Err(DiffError(
            "diff exceeds GitHub's comment limit; full report was printed to stdout".into(),
        ));
    }
    let endpoint = format!("repos/{repo}/issues/{pr}/comments");
    let response = command(
        dir,
        "gh",
        &["api", &endpoint, "--paginate", "--slurp"],
        None,
    )?;
    let pages: Value = serde_json::from_str(&response)
        .map_err(|e| DiffError(format!("invalid GitHub response: {e}")))?;
    let (method, endpoint) = match existing_comment(&pages, &marker)? {
        Some(id) => ("PATCH", format!("repos/{repo}/issues/comments/{id}")),
        None => ("POST", endpoint),
    };
    let input = serde_json::to_vec(&json!({"body": body})).expect("serialize comment");
    command(
        dir,
        "gh",
        &["api", &endpoint, "--method", method, "--input", "-"],
        Some(&input),
    )?;
    eprintln!("trigp: updated spec diff comment on {repo}#{pr}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_exact_marker_across_pages_and_preserves_other_comments() {
        let m = marker("spec/invariants");
        let pages = json!([[{"id":1,"body":"unrelated"}, {"id":2,"body":format!("quote {m}")}], [{"id":3,"body":format!("{m}\nold report")}]]);
        assert_eq!(existing_comment(&pages, &m).unwrap(), Some(3));
        assert_eq!(existing_comment(&pages, &marker("other")).unwrap(), None);
    }
}
