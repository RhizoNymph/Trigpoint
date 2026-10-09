use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "trigp-diff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let repo = Self(path);
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.commit();
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn commit(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "--allow-empty", "-m", "fixture"]);
    }
    fn spec(&self, dir: &str, id: &str, statement: &str) {
        fs::create_dir_all(self.0.join(dir)).unwrap();
        fs::write(
            self.0.join(dir).join(format!("{id}.toml")),
            format!("[invariant]\nid={id:?}\nstatement={statement:?}\nkind='system'\n"),
        )
        .unwrap();
    }
    fn cli(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_trigp"));
        cmd.current_dir(&self.0)
            .args(["spec", "diff"])
            .env_remove("GITHUB_EVENT_PATH")
            .env_remove("GITHUB_BASE_REF")
            .env_remove("GITHUB_REPOSITORY");
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cli().args(args).output().unwrap()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn merge_base_committed_content_all_categories_and_exit_policy() {
    let repo = Repo::new();
    repo.spec("spec/invariants", "removed", "old");
    repo.spec("spec/invariants", "changed", "before");
    repo.commit();
    repo.git(&["switch", "-c", "feature"]);
    repo.spec("spec/invariants", "created", "new");
    repo.spec("spec/invariants", "changed", "after");
    fs::remove_file(repo.0.join("spec/invariants/removed.toml")).unwrap();
    repo.commit();
    repo.git(&["switch", "main"]);
    repo.spec("spec/invariants", "base-only", "not in branch diff");
    repo.commit();
    repo.git(&["switch", "feature"]);
    repo.spec("spec/invariants", "dirty", "ignored");
    let output = repo.run(&[]);
    success(&output);
    let report = stdout(&output);
    assert!(report.contains("| created | ✓ | — |"), "{report}");
    assert!(report.contains("| removed | — | ✓ |"));
    assert!(report.contains("statement = \"after\""));
    assert!(report.contains("statement = \"before\""));
    assert!(!report.contains("base-only"));
    assert!(!report.contains("dirty"));
    assert_eq!(repo.run(&["--fail-on-change"]).status.code(), Some(1));
    assert_eq!(repo.git(&["branch", "--show-current"]), "feature");
}

#[test]
fn missing_directories_subdirectories_and_bad_refs() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "feature"]);
    repo.spec("project/spec/invariants", "new", "claim");
    repo.commit();
    let output = repo.run(&["-C", "project", "--base", "main"]);
    success(&output);
    assert!(stdout(&output).contains("| new | ✓ |"));
    let output = repo.run(&["--spec-dir", "absent"]);
    success(&output);
    assert!(stdout(&output).contains("No invariant changes."));
    assert!(!repo.run(&["--base", "missing"]).status.success());
    assert!(!repo.run(&["--base", "--help"]).status.success());
    assert!(!repo.run(&["--spec-dir", "../outside"]).status.success());
}

#[test]
fn malformed_committed_spec_fails_but_dirty_files_do_not_matter() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "feature"]);
    repo.spec("spec/invariants", "a", "ok");
    repo.commit();
    fs::write(repo.0.join("spec/invariants/a.toml"), "broken").unwrap();
    success(&repo.run(&[]));
    repo.commit();
    let result = repo.run(&[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("a.toml"));
}

#[cfg(unix)]
#[test]
fn pr_event_uses_head_not_checkout_and_comment_is_updated_on_second_run() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    let base = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["switch", "-c", "feature"]);
    repo.spec("spec/invariants", "new", "hello | <world>");
    repo.commit();
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["switch", "main"]);
    let event = repo.0.join("event.json");
    fs::write(&event, serde_json::json!({"repository":{"full_name":"owner/repo"},"pull_request":{"number":42,"base":{"sha":base},"head":{"sha":head}}}).to_string()).unwrap();
    let bin = repo.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
state = Path(os.environ['MOCK_STATE'])
args = sys.argv[1:]
with state.with_suffix('.log').open('a') as f:
    f.write(json.dumps(args) + '\n')
assert args[0] == 'api'
if '--method' not in args:
    assert args == ['api', 'repos/owner/repo/issues/42/comments', '--paginate', '--slurp']
    previous = json.loads(state.read_text()) if state.exists() else None
    print(json.dumps([[{'id': 1, 'body': 'unrelated'}], [previous] if previous else []]))
else:
    method = args[args.index('--method') + 1]
    assert method == ('PATCH' if state.exists() else 'POST')
    assert args[1] == ('repos/owner/repo/issues/comments/99' if state.exists() else 'repos/owner/repo/issues/42/comments')
    body = json.load(sys.stdin)['body']
    state.write_text(json.dumps({'id':99, 'body':body}))
    print('{}')
"#).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let state = repo.0.join("comment.json");
    for gated in [false, true] {
        let mut command = repo.cli();
        if gated {
            command.arg("--fail-on-change");
        }
        let output = command
            .arg("--comment")
            .env("GITHUB_EVENT_PATH", &event)
            .env("PATH", &path)
            .env("MOCK_STATE", &state)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if gated { 1 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout(&output).contains("| new | ✓ |"));
    }
    // A no-change rerun must clear the old table, rather than leave stale rows.
    let output = repo
        .cli()
        .args(["--comment", "--head", "main"])
        .env("GITHUB_EVENT_PATH", &event)
        .env("PATH", &path)
        .env("MOCK_STATE", &state)
        .output()
        .unwrap();
    success(&output);
    assert!(
        fs::read_to_string(state)
            .unwrap()
            .contains("No invariant changes.")
    );
    fs::write(&gh, "#!/bin/sh\necho 'permission denied' >&2\nexit 1\n").unwrap();
    let output = repo
        .cli()
        .arg("--comment")
        .env("GITHUB_EVENT_PATH", &event)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("permission denied"));
    assert!(stdout(&output).contains("| new | ✓ |"));
}

#[test]
fn spec_dir_can_be_repository_root() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "feature"]);
    repo.spec(".", "a", "root invariant");
    repo.commit();
    let output = repo.run(&["--spec-dir", "."]);
    success(&output);
    assert!(stdout(&output).contains("| a | ✓ |"));
}
