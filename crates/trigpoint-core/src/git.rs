//! Answering "has this changed since that commit?" for review staleness.
//!
//! A review pinned to a commit is only as good as the sources it reviewed.
//! The oracle is a trait so the checker can be tested without a repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("failed to run git: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("{dir} is not inside a git repository")]
    NotARepository { dir: PathBuf },
    #[error("git failed: {stderr}")]
    Failed { stderr: String },
}

pub trait ChangeOracle {
    /// Whether `commit` names a commit in this repository.
    fn commit_exists(&self, commit: &str) -> Result<bool, GitError>;
    /// Whether anything under `paths` differs between `commit` and the
    /// working tree.
    fn changed_since(&self, commit: &str, paths: &[PathBuf]) -> Result<bool, GitError>;
}

/// The real thing, shelling out to `git` in a directory.
pub struct GitOracle {
    dir: PathBuf,
}

impl GitOracle {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
        }
    }

    fn git(&self, args: &[&str]) -> Result<std::process::Output, GitError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .output()
            .map_err(GitError::Spawn)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Err(GitError::NotARepository {
                dir: self.dir.clone(),
            });
        }
        Ok(output)
    }
}

impl ChangeOracle for GitOracle {
    fn commit_exists(&self, commit: &str) -> Result<bool, GitError> {
        let spec = format!("{commit}^{{commit}}");
        let output = self.git(&["cat-file", "-e", &spec])?;
        Ok(output.status.success())
    }

    fn changed_since(&self, commit: &str, paths: &[PathBuf]) -> Result<bool, GitError> {
        let mut args = vec!["diff", "--quiet", commit, "--"];
        let rendered: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        args.extend(rendered.iter().map(String::as_str));
        let output = self.git(&args)?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(GitError::Failed {
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
        }
    }
}
