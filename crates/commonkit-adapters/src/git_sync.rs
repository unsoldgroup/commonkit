use std::{fmt, path::PathBuf, process::Command};

use commonkit_core::is_forbidden_path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommandOutput {
    pub stdout: String,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("git command failed: {message}")]
pub struct GitCommandError {
    pub message: String,
}

/// Executes `git` directly. Arguments are passed as argv and are never interpreted by a shell.
pub trait GitCommandRunner {
    fn run(&mut self, args: &[String]) -> Result<GitCommandOutput, GitCommandError>;
}

#[derive(Debug, Clone)]
pub struct ProcessGitRunner {
    repository: PathBuf,
}

impl ProcessGitRunner {
    pub fn new(repository: impl Into<PathBuf>) -> Self {
        Self {
            repository: repository.into(),
        }
    }
}

impl GitCommandRunner for ProcessGitRunner {
    fn run(&mut self, args: &[String]) -> Result<GitCommandOutput, GitCommandError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repository)
            .args(args)
            .output()
            .map_err(|error| GitCommandError {
                message: error.to_string(),
            })?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            return Err(GitCommandError {
                message: message.chars().take(512).collect(),
            });
        }
        String::from_utf8(output.stdout)
            .map(|stdout| GitCommandOutput { stdout })
            .map_err(|_| GitCommandError {
                message: "git emitted non-UTF-8 output".into(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRevision(String);

impl GitRevision {
    fn parse(value: &str) -> Result<Self, GitSyncError> {
        let value = value.trim();
        if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GitSyncError::InvalidRevision(value.into()));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSyncDisposition {
    Clean,
    Dirty,
    Ahead,
    Behind,
    Diverged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSyncStatus {
    pub disposition: GitSyncDisposition,
    pub revision: GitRevision,
    pub upstream_revision: GitRevision,
    pub branch: String,
    pub remote_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastForwardPolicy {
    pub enabled: bool,
}

#[derive(Debug, Error)]
pub enum GitSyncError {
    #[error(transparent)]
    Command(#[from] GitCommandError),
    #[error("repository revision is not a full object ID: {0}")]
    InvalidRevision(String),
    #[error("repository has no checked-out branch")]
    DetachedHead,
    #[error("remote source is not trusted: expected {expected}, observed {observed}")]
    UntrustedRemote { expected: String, observed: String },
    #[error("invalid ahead/behind response: {0}")]
    InvalidDivergence(String),
    #[error("portable repository tree is unsafe at {path}: {reason}")]
    UnsafeTree { path: String, reason: &'static str },
    #[error("automatic fast-forward is disabled")]
    FastForwardDisabled,
    #[error("organization policy rejected the fetched revision")]
    OrganizationPolicyRejected,
    #[error("fast-forward requires a clean behind-only repository, observed {0:?}")]
    FastForwardBlocked(GitSyncDisposition),
}

pub struct GitRepository<R> {
    runner: R,
    trusted_remote_url: String,
    remote: String,
}

impl<R: GitCommandRunner> GitRepository<R> {
    pub fn new(
        runner: R,
        trusted_remote_url: impl Into<String>,
        remote: impl Into<String>,
    ) -> Self {
        Self {
            runner,
            trusted_remote_url: trusted_remote_url.into(),
            remote: remote.into(),
        }
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }

    pub fn runner_mut(&mut self) -> &mut R {
        &mut self.runner
    }

    pub fn into_runner(self) -> R {
        self.runner
    }

    pub fn inspect(&mut self, fetch: bool) -> Result<GitSyncStatus, GitSyncError> {
        let remote_key = format!("remote.{}.url", self.remote);
        let remote_url = self
            .git(&["config", "--get", &remote_key])?
            .trim()
            .to_owned();
        if remote_url != self.trusted_remote_url {
            return Err(GitSyncError::UntrustedRemote {
                expected: self.trusted_remote_url.clone(),
                observed: remote_url,
            });
        }
        if fetch {
            let remote = self.remote.clone();
            self.git(&["fetch", "--prune", "--no-tags", &remote])?;
        }
        let dirty = !self
            .git(&["status", "--porcelain=v1", "--untracked-files=all"])?
            .trim()
            .is_empty();
        let revision = GitRevision::parse(&self.git(&["rev-parse", "--verify", "HEAD"])?)?;
        let branch = self
            .git(&["symbolic-ref", "--quiet", "--short", "HEAD"])?
            .trim()
            .to_owned();
        if branch.is_empty() {
            return Err(GitSyncError::DetachedHead);
        }
        let upstream_ref = format!("refs/remotes/{}/{}", self.remote, branch);
        let upstream_revision =
            GitRevision::parse(&self.git(&["rev-parse", "--verify", &upstream_ref])?)?;
        let counts = self.git(&[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{upstream_ref}"),
        ])?;
        let (ahead, behind) = parse_divergence(&counts)?;
        let disposition = if dirty {
            GitSyncDisposition::Dirty
        } else {
            match (ahead, behind) {
                (0, 0) => GitSyncDisposition::Clean,
                (_, 0) => GitSyncDisposition::Ahead,
                (0, _) => GitSyncDisposition::Behind,
                (_, _) => GitSyncDisposition::Diverged,
            }
        };
        validate_tree(&self.git(&["ls-tree", "-r", "--full-tree", "HEAD"])?)?;
        Ok(GitSyncStatus {
            disposition,
            revision,
            upstream_revision,
            branch,
            remote_url,
        })
    }

    pub fn fast_forward(
        &mut self,
        policy: FastForwardPolicy,
        organization_validates: impl FnOnce(&GitSyncStatus) -> bool,
    ) -> Result<GitSyncStatus, GitSyncError> {
        if !policy.enabled {
            return Err(GitSyncError::FastForwardDisabled);
        }
        let status = self.inspect(true)?;
        if status.disposition != GitSyncDisposition::Behind {
            return Err(GitSyncError::FastForwardBlocked(status.disposition));
        }
        let upstream_ref = format!("refs/remotes/{}/{}", self.remote, status.branch);
        validate_tree(&self.git(&["ls-tree", "-r", "--full-tree", &upstream_ref])?)?;
        if !organization_validates(&status) {
            return Err(GitSyncError::OrganizationPolicyRejected);
        }
        self.git(&["merge", "--ff-only", &upstream_ref])?;
        Ok(GitSyncStatus {
            disposition: GitSyncDisposition::Clean,
            revision: status.upstream_revision.clone(),
            upstream_revision: status.upstream_revision,
            ..status
        })
    }

    fn git(&mut self, args: &[&str]) -> Result<String, GitSyncError> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        Ok(self.runner.run(&args)?.stdout)
    }
}

fn parse_divergence(value: &str) -> Result<(u64, u64), GitSyncError> {
    let mut values = value.split_whitespace();
    let ahead = values.next().and_then(|part| part.parse().ok());
    let behind = values.next().and_then(|part| part.parse().ok());
    match (ahead, behind, values.next()) {
        (Some(ahead), Some(behind), None) => Ok((ahead, behind)),
        _ => Err(GitSyncError::InvalidDivergence(value.trim().into())),
    }
}

fn validate_tree(tree: &str) -> Result<(), GitSyncError> {
    for line in tree.lines().filter(|line| !line.is_empty()) {
        let (metadata, path) = line
            .split_once('\t')
            .ok_or_else(|| GitSyncError::UnsafeTree {
                path: "<unknown>".into(),
                reason: "malformed Git tree entry",
            })?;
        let mode = metadata.split_whitespace().next().unwrap_or_default();
        let path = path.replace('\\', "/");
        let reason = match mode {
            "120000" => Some("symbolic links are not portable-store content"),
            "160000" => Some("Git submodules are not portable-store content"),
            _ if is_forbidden_path(&path) => {
                Some("secret or mutable live-database paths are forbidden")
            }
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(GitSyncError::UnsafeTree { path, reason });
        }
    }
    Ok(())
}

impl fmt::Debug for GitRepository<ProcessGitRunner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitRepository")
            .field("runner", &self.runner)
            .field("trusted_remote_url", &self.trusted_remote_url)
            .field("remote", &self.remote)
            .finish()
    }
}
