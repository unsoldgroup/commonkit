#[cfg(target_os = "linux")]
use commonkit_contracts::NetworkPolicy;
use commonkit_contracts::{ExecutionManifest, Sha256Digest};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorMode {
    ProcessGroup,
    SystemdScope,
    #[cfg(target_os = "linux")]
    LinuxSandbox,
}
pub struct ProcessSupervisor {
    mode: SupervisorMode,
    diagnostic_root: PathBuf,
}
pub struct RunningProcess {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
    started: Instant,
    timeout: Duration,
    disk_root: PathBuf,
    initial_disk_bytes: u64,
    disk_limit_bytes: u64,
    systemd_unit: Option<String>,
}
#[derive(Debug)]
pub struct ProcessOutcome {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

impl ProcessSupervisor {
    pub fn new(
        mode: SupervisorMode,
        diagnostic_root: impl AsRef<Path>,
    ) -> Result<Self, SupervisorError> {
        std::fs::create_dir_all(diagnostic_root.as_ref())?;
        Ok(Self {
            mode,
            diagnostic_root: diagnostic_root.as_ref().into(),
        })
    }
    pub fn spawn(
        &self,
        manifest: &ExecutionManifest,
        workspace: &Path,
    ) -> Result<RunningProcess, SupervisorError> {
        manifest.validate()?;
        let workdir = workspace.join(manifest.workdir.as_str());
        let canonical_workspace = workspace.canonicalize()?;
        let canonical_workdir = workdir.canonicalize()?;
        if !canonical_workdir.starts_with(&canonical_workspace) {
            return Err(SupervisorError::WorkspaceEscape);
        }
        verify_workspace(manifest, &canonical_workspace)?;
        let initial_disk_bytes = directory_size(&canonical_workspace)?;
        let id = Uuid::new_v4();
        let systemd_unit = (self.mode == SupervisorMode::SystemdScope)
            .then(|| format!("commonkit-execution-{id}.scope"));
        let stdout = self.diagnostic_root.join(format!("{id}.stdout"));
        let stderr = self.diagnostic_root.join(format!("{id}.stderr"));
        let mut command = match self.mode {
            SupervisorMode::SystemdScope => systemd_command(
                manifest,
                &canonical_workspace,
                &canonical_workdir,
                systemd_unit.as_deref().expect("systemd unit"),
            )?,
            #[cfg(target_os = "linux")]
            SupervisorMode::LinuxSandbox => {
                sandbox_command(manifest, &canonical_workspace, &canonical_workdir)?
            }
            SupervisorMode::ProcessGroup => {
                let mut command = Command::new(&manifest.argv[0]);
                command.args(&manifest.argv[1..]);
                command
            }
        };
        command
            .current_dir(canonical_workdir)
            .stdin(Stdio::null())
            .stdout(File::create(&stdout)?)
            .stderr(File::create(&stderr)?);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let file_limit = manifest.resources.disk_mib.saturating_mul(1024 * 1024);
            unsafe {
                command.pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let limit = libc::rlimit {
                        rlim_cur: file_limit as libc::rlim_t,
                        rlim_max: file_limit as libc::rlim_t,
                    };
                    if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        Ok(RunningProcess {
            child: command.spawn()?,
            stdout,
            stderr,
            started: Instant::now(),
            timeout: Duration::from_secs(manifest.timeout_seconds),
            disk_root: canonical_workspace,
            initial_disk_bytes,
            disk_limit_bytes: manifest.resources.disk_mib.saturating_mul(1024 * 1024),
            systemd_unit,
        })
    }
}
impl RunningProcess {
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, SupervisorError> {
        if self.started.elapsed() >= self.timeout {
            self.kill_group()?;
            return Err(SupervisorError::TimedOut);
        }
        if directory_size(&self.disk_root)?.saturating_sub(self.initial_disk_bytes)
            > self.disk_limit_bytes
        {
            self.kill_group()?;
            return Err(SupervisorError::DiskLimitExceeded);
        }
        Ok(self.child.try_wait()?)
    }
    pub fn wait(mut self, secrets: &[String]) -> Result<ProcessOutcome, SupervisorError> {
        loop {
            if let Some(status) = self.try_wait()? {
                return self.finish(status, secrets);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn finish(
        mut self,
        status: ExitStatus,
        secrets: &[String],
    ) -> Result<ProcessOutcome, SupervisorError> {
        let _ = self.child.try_wait()?;
        self.outcome(status, secrets)
    }
    pub fn cancel(
        mut self,
        grace: Duration,
        secrets: &[String],
    ) -> Result<ProcessOutcome, SupervisorError> {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        #[cfg(not(unix))]
        self.child.kill()?;
        let deadline = Instant::now() + grace;
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
                #[cfg(not(unix))]
                self.child.kill()?;
                break self.child.wait()?;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        self.outcome(status, secrets)
    }
    fn kill_group(&mut self) -> Result<(), SupervisorError> {
        self.kill_systemd_unit("SIGKILL")?;
        #[cfg(unix)]
        unsafe {
            if libc::kill(-(self.child.id() as i32), libc::SIGKILL) == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error.into());
                }
            }
        }
        #[cfg(not(unix))]
        self.child.kill()?;
        let _ = self.child.wait()?;
        Ok(())
    }
    fn kill_systemd_unit(&self, signal: &str) -> Result<(), SupervisorError> {
        if let Some(unit) = &self.systemd_unit {
            let status = Command::new("systemctl")
                .args([
                    "--user",
                    "kill",
                    "--kill-whom=all",
                    "--signal",
                    signal,
                    unit,
                ])
                .status()?;
            if !status.success() {
                return Err(SupervisorError::SystemdControlFailed);
            }
        }
        Ok(())
    }
    fn outcome(
        &self,
        status: ExitStatus,
        secrets: &[String],
    ) -> Result<ProcessOutcome, SupervisorError> {
        Ok(ProcessOutcome {
            status,
            stdout: read_redacted(&self.stdout, secrets)?,
            stderr: read_redacted(&self.stderr, secrets)?,
            elapsed: self.started.elapsed(),
        })
    }
}
fn read_redacted(path: &Path, secrets: &[String]) -> Result<String, SupervisorError> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024).read_to_end(&mut bytes)?;
    let mut value = String::from_utf8_lossy(&bytes).into_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(secret, "[REDACTED]");
    }
    Ok(value)
}

pub fn workspace_bundle_digest(workspace: &Path) -> Result<Sha256Digest, SupervisorError> {
    let root = workspace.canonicalize()?;
    let mut entries = Vec::new();
    collect_entries(&root, &root, &mut entries)?;
    entries.sort();
    let mut hash = Sha256::new();
    hash.update(b"commonkit.execution-workspace.v1\0");
    for relative in entries {
        let path = root.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(SupervisorError::WorkspaceSymlink(relative));
        }
        hash.update(relative.to_string_lossy().as_bytes());
        hash.update([0]);
        if metadata.is_file() {
            hash.update(b"file\0");
            hash.update(std::fs::read(path)?);
        } else {
            hash.update(b"directory\0");
        }
        hash.update([0]);
    }
    Sha256Digest::parse(format!("sha256:{:x}", hash.finalize())).map_err(SupervisorError::Contract)
}

fn verify_workspace(manifest: &ExecutionManifest, workspace: &Path) -> Result<(), SupervisorError> {
    let output = Command::new("git")
        .args([
            "-C",
            path_arg(workspace)?,
            "rev-parse",
            "--verify",
            "HEAD^{commit}",
        ])
        .output()?;
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || revision != manifest.repository_revision.as_str() {
        return Err(SupervisorError::RepositoryRevisionMismatch);
    }
    let remote = Command::new("git")
        .args(["-C", path_arg(workspace)?, "remote", "get-url", "origin"])
        .output()?;
    if !remote.status.success()
        || String::from_utf8_lossy(&remote.stdout).trim() != manifest.repository
    {
        return Err(SupervisorError::RepositoryMismatch);
    }
    let status = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
            path_arg(workspace)?,
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ])
        .output()?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err(SupervisorError::WorkspaceNotClean);
    }
    let observed_bundle = workspace_bundle_digest(workspace)?;
    if let Some(expected) = &manifest.workspace_bundle_digest
        && observed_bundle != *expected
    {
        return Err(SupervisorError::WorkspaceBundleMismatch);
    }
    Ok(())
}

fn collect_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<PathBuf>,
) -> Result<(), SupervisorError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| SupervisorError::WorkspaceEscape)?
            .to_path_buf();
        if relative
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == ".git")
        {
            continue;
        }
        entries.push(relative.clone());
        if entry.file_type()?.is_dir() {
            collect_entries(root, &entry.path(), entries)?;
        }
    }
    Ok(())
}

fn directory_size(root: &Path) -> Result<u64, SupervisorError> {
    let mut total = 0_u64;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn path_arg(path: &Path) -> Result<&str, SupervisorError> {
    path.to_str().ok_or(SupervisorError::NonUtf8Path)
}

#[cfg(target_os = "linux")]
fn systemd_command(
    manifest: &ExecutionManifest,
    workspace: &Path,
    workdir: &Path,
    unit: &str,
) -> Result<Command, SupervisorError> {
    let mut command = Command::new("systemd-run");
    let cpu = manifest.resources.cpu_millis.div_ceil(10);
    command.args([
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--unit",
        unit,
        "-p",
        &format!("RuntimeMaxSec={}s", manifest.timeout_seconds),
        "-p",
        &format!("MemoryMax={}M", manifest.resources.memory_mib),
        "-p",
        &format!("CPUQuota={cpu}%"),
        "--",
    ]);
    command.args(sandbox_args(manifest, workspace, workdir)?);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn sandbox_command(
    manifest: &ExecutionManifest,
    workspace: &Path,
    workdir: &Path,
) -> Result<Command, SupervisorError> {
    let mut command = Command::new("bwrap");
    let mut args = sandbox_args(manifest, workspace, workdir)?;
    args.remove(0);
    command.args(args);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn sandbox_args<'a>(
    manifest: &'a ExecutionManifest,
    workspace: &'a Path,
    workdir: &'a Path,
) -> Result<Vec<&'a str>, SupervisorError> {
    if manifest.network_policy == NetworkPolicy::Restricted {
        return Err(SupervisorError::UnsupportedNetworkPolicy);
    }
    if Command::new("bwrap").arg("--version").output().is_err() {
        return Err(SupervisorError::IsolationUnavailable);
    }
    let mut args = vec!["bwrap", "--die-with-parent", "--new-session"];
    if manifest.network_policy == NetworkPolicy::Deny {
        args.push("--unshare-net");
    }
    args.extend(["--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev"]);
    let binding = if manifest.repository_write {
        "--bind"
    } else {
        "--ro-bind"
    };
    args.extend([binding, path_arg(workspace)?, path_arg(workspace)?]);
    args.extend([
        "--tmpfs",
        "/tmp",
        "--chdir",
        path_arg(workdir)?,
        "--",
        &manifest.argv[0],
    ]);
    args.extend(manifest.argv[1..].iter().map(String::as_str));
    Ok(args)
}

#[cfg(not(target_os = "linux"))]
fn systemd_command(
    _manifest: &ExecutionManifest,
    _workspace: &Path,
    _workdir: &Path,
    _unit: &str,
) -> Result<Command, SupervisorError> {
    Err(SupervisorError::UnsupportedIsolation)
}
#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("process I/O failed")]
    Io(#[from] std::io::Error),
    #[error("execution contract failed")]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error("workdir escapes workspace")]
    WorkspaceEscape,
    #[error("workspace repository revision does not match the execution manifest")]
    RepositoryRevisionMismatch,
    #[error("workspace repository does not match the execution manifest")]
    RepositoryMismatch,
    #[error("workspace is not a clean materialization of its pinned repository revision")]
    WorkspaceNotClean,
    #[error("workspace bundle digest does not match the execution manifest")]
    WorkspaceBundleMismatch,
    #[error("workspace contains a symlink: {0}")]
    WorkspaceSymlink(PathBuf),
    #[error("execution timed out")]
    TimedOut,
    #[error("execution exceeded its disk limit")]
    DiskLimitExceeded,
    #[error("restricted network policy has no enforceable destination contract")]
    UnsupportedNetworkPolicy,
    #[error("production execution isolation is supported only on Linux")]
    UnsupportedIsolation,
    #[error("required Linux isolation executable is unavailable")]
    IsolationUnavailable,
    #[error("execution path is not UTF-8")]
    NonUtf8Path,
    #[error("systemd failed to terminate the execution scope")]
    SystemdControlFailed,
}
