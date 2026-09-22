//! The Rust target: orchestrating cargo-dylint so users don't have to know the
//! environment folklore (DYLINT_RUSTFLAGS vs RUSTFLAGS, cache busting when MIR
//! flags change).

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

use super::LintError;

/// Flag that makes dependency bodies traversable by triglint. Passed via
/// DYLINT_RUSTFLAGS (never RUSTFLAGS, which leaks into cargo-dylint's
/// stable-toolchain probes and breaks library discovery).
pub const ALWAYS_ENCODE_MIR: &str = "-Zalways-encode-mir";

/// Runs `cargo dylint --all`, returning its exit code.
pub fn run(
    dir: &Path,
    fresh: bool,
    deps_mir: bool,
    cargo_args: &[OsString],
) -> Result<i32, LintError> {
    ensure_installed(dir)?;

    if fresh {
        clear_analysis_cache(dir)?;
    }

    let flags = merged_dylint_rustflags(env::var("DYLINT_RUSTFLAGS").ok().as_deref(), deps_mir);

    let mut command = Command::new("cargo");
    command.arg("dylint").arg("--all").current_dir(dir);
    if let Some(flags) = flags {
        command.env("DYLINT_RUSTFLAGS", flags);
    }
    if !cargo_args.is_empty() {
        command.arg("--");
        command.args(cargo_args);
    }
    let status = command.status().map_err(|source| LintError::Spawn {
        what: "cargo dylint",
        source,
    })?;
    Ok(status.code().unwrap_or(1))
}

/// Appends -Zalways-encode-mir to any user-provided DYLINT_RUSTFLAGS,
/// without duplicating it. `None` means "leave the variable unset".
pub fn merged_dylint_rustflags(existing: Option<&str>, deps_mir: bool) -> Option<String> {
    if !deps_mir {
        return existing.map(str::to_owned);
    }
    match existing {
        None => Some(ALWAYS_ENCODE_MIR.to_owned()),
        Some(flags) if flags.split_whitespace().any(|f| f == ALWAYS_ENCODE_MIR) => {
            Some(flags.to_owned())
        }
        Some(flags) => Some(format!("{flags} {ALWAYS_ENCODE_MIR}")),
    }
}

/// True when a `Cargo.toml` at or above `start` declares dylint libraries —
/// the marker that this workspace has a Rust lint target at all.
pub fn has_metadata(start: &Path) -> bool {
    let mut dir = start;
    loop {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file()
            && let Ok(text) = fs::read_to_string(&manifest)
            && manifest_declares_dylint(&text)
        {
            return true;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return false,
        }
    }
}

/// `[workspace.metadata.dylint]` or `[package.metadata.dylint]` in a manifest.
pub fn manifest_declares_dylint(text: &str) -> bool {
    let Ok(document) = text.parse::<toml::Table>() else {
        return false;
    };
    ["workspace", "package"].iter().any(|table| {
        document
            .get(*table)
            .and_then(|t| t.get("metadata"))
            .and_then(|m| m.get("dylint"))
            .is_some()
    })
}

fn ensure_installed(dir: &Path) -> Result<(), LintError> {
    let output = Command::new("cargo")
        .args(["dylint", "--version"])
        .current_dir(dir)
        .output()
        .map_err(|source| LintError::Spawn {
            what: "cargo dylint --version",
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(LintError::CargoDylintMissing)
    }
}

/// Removes dylint's per-toolchain analysis target dir (not the built lint
/// libraries), forcing recompilation of the analyzed workspace.
fn clear_analysis_cache(dir: &Path) -> Result<(), LintError> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(dir)
        .output()
        .map_err(|source| LintError::Spawn {
            what: "cargo metadata",
            source,
        })?;
    if !output.status.success() {
        return Err(LintError::Metadata {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let Some(target_dir) = metadata["target_directory"].as_str() else {
        return Err(LintError::Metadata {
            stderr: "metadata has no target_directory".to_owned(),
        });
    };
    let cache = Path::new(target_dir).join("dylint").join("target");
    if cache.exists() {
        fs::remove_dir_all(&cache).map_err(|source| LintError::ClearCache {
            path: cache.clone(),
            source,
        })?;
        eprintln!(
            "trigp: cleared dylint analysis cache at {}",
            cache.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_flag_into_empty_env() {
        assert_eq!(
            merged_dylint_rustflags(None, true).as_deref(),
            Some(ALWAYS_ENCODE_MIR)
        );
    }

    #[test]
    fn appends_flag_to_existing_flags() {
        assert_eq!(
            merged_dylint_rustflags(Some("-Zthreads=2"), true).as_deref(),
            Some("-Zthreads=2 -Zalways-encode-mir")
        );
    }

    #[test]
    fn does_not_duplicate_flag() {
        assert_eq!(
            merged_dylint_rustflags(Some("-Zalways-encode-mir"), true).as_deref(),
            Some(ALWAYS_ENCODE_MIR)
        );
    }

    #[test]
    fn no_deps_mir_leaves_env_untouched() {
        assert_eq!(merged_dylint_rustflags(None, false), None);
        assert_eq!(
            merged_dylint_rustflags(Some("-Zthreads=2"), false).as_deref(),
            Some("-Zthreads=2")
        );
    }

    #[test]
    fn detects_dylint_metadata_in_a_manifest() {
        assert!(manifest_declares_dylint(
            "[workspace]\nmembers = []\n\n[workspace.metadata.dylint]\nlibraries = []\n"
        ));
        assert!(manifest_declares_dylint(
            "[package]\nname = \"x\"\n\n[package.metadata.dylint]\nlibraries = []\n"
        ));
        assert!(!manifest_declares_dylint("[package]\nname = \"x\"\n"));
        assert!(!manifest_declares_dylint("not toml ["));
    }
}
