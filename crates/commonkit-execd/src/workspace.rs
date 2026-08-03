//! Per-job git workspace preparation.
//!
//! Each repository is cached once as a bare mirror; every job checks out its own
//! detached worktree of that mirror. Preparation stays outside durable state: a
//! prepared workspace is reproducible from the manifest alone.

use commonkit_contracts::ExecutionManifest;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prepares a workspace for `job_id` at the manifest's exact revision and returns its path.
///
/// The caller is responsible for checking the execution policy first; this function
/// fetches from `manifest.repository` and must never run for a denied repository.
pub fn prepare(
    manifest: &ExecutionManifest,
    root: &Path,
    job_id: &str,
) -> Result<PathBuf, WorkspaceError> {
    if !valid_job_id(job_id) {
        return Err(WorkspaceError::InvalidJobId(job_id.to_owned()));
    }
    let mirror = mirror_path(root, &manifest.repository);
    if mirror.join("HEAD").exists() {
        git(&["-C", path_arg(&mirror)?, "remote", "update", "--prune"])?;
    } else {
        std::fs::create_dir_all(mirror.parent().expect("mirror parent"))?;
        git(&[
            "clone",
            "--mirror",
            &manifest.repository,
            path_arg(&mirror)?,
        ])?;
    }
    let worktree = job_path(root, job_id);
    if worktree.exists() {
        remove(root, &manifest.repository, job_id)?;
    }
    std::fs::create_dir_all(worktree.parent().expect("worktree parent"))?;
    git(&[
        "-C",
        path_arg(&mirror)?,
        "worktree",
        "add",
        "--detach",
        path_arg(&worktree)?,
        manifest.repository_revision.as_str(),
    ])?;
    Ok(worktree)
}

/// Removes a prepared worktree. Safe to call when the worktree is already gone.
pub fn remove(root: &Path, repository: &str, job_id: &str) -> Result<(), WorkspaceError> {
    if !valid_job_id(job_id) {
        return Err(WorkspaceError::InvalidJobId(job_id.to_owned()));
    }
    let mirror = mirror_path(root, repository);
    let worktree = job_path(root, job_id);
    if worktree.exists() {
        // Best effort: a half-created worktree may not be registered with the mirror.
        let _ = git(&[
            "-C",
            path_arg(&mirror)?,
            "worktree",
            "remove",
            "--force",
            path_arg(&worktree)?,
        ]);
        if worktree.exists() {
            std::fs::remove_dir_all(&worktree)?;
        }
    }
    let _ = git(&["-C", path_arg(&mirror)?, "worktree", "prune"]);
    Ok(())
}

fn mirror_path(root: &Path, repository: &str) -> PathBuf {
    let digest = Sha256::digest(repository.as_bytes());
    root.join("mirrors").join(format!("{digest:x}.git"))
}

fn job_path(root: &Path, job_id: &str) -> PathBuf {
    root.join("jobs").join(job_id)
}

fn valid_job_id(job_id: &str) -> bool {
    !job_id.is_empty()
        && job_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn path_arg(path: &Path) -> Result<&str, WorkspaceError> {
    path.to_str()
        .ok_or_else(|| WorkspaceError::NonUtf8Path(path.to_path_buf()))
}

fn git(args: &[&str]) -> Result<(), WorkspaceError> {
    let output = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(WorkspaceError::Git {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("job ID is not a safe path component: {0}")]
    InvalidJobId(String),
    #[error("workspace path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("git {command} failed: {stderr}")]
    Git { command: String, stderr: String },
    #[error("workspace filesystem operation failed")]
    Io(#[from] std::io::Error),
}
