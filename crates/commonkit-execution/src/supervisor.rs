use commonkit_contracts::ExecutionManifest;
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
        let id = Uuid::new_v4();
        let stdout = self.diagnostic_root.join(format!("{id}.stdout"));
        let stderr = self.diagnostic_root.join(format!("{id}.stderr"));
        let mut command = if self.mode == SupervisorMode::SystemdScope {
            let mut command = Command::new("systemd-run");
            let cpu = manifest.resources.cpu_millis.div_ceil(10);
            command.args([
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "-p",
                &format!("MemoryMax={}M", manifest.resources.memory_mib),
                "-p",
                &format!("CPUQuota={cpu}%"),
                "--",
                &manifest.argv[0],
            ]);
            command.args(&manifest.argv[1..]);
            command
        } else {
            let mut command = Command::new(&manifest.argv[0]);
            command.args(&manifest.argv[1..]);
            command
        };
        command
            .current_dir(canonical_workdir)
            .stdin(Stdio::null())
            .stdout(File::create(&stdout)?)
            .stderr(File::create(&stderr)?);
        #[cfg(unix)]
        if self.mode == SupervisorMode::ProcessGroup {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        Ok(RunningProcess {
            child: command.spawn()?,
            stdout,
            stderr,
            started: Instant::now(),
        })
    }
}
impl RunningProcess {
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, SupervisorError> {
        Ok(self.child.try_wait()?)
    }
    pub fn wait(mut self, secrets: &[String]) -> Result<ProcessOutcome, SupervisorError> {
        let status = self.child.wait()?;
        self.outcome(status, secrets)
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
#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("process I/O failed")]
    Io(#[from] std::io::Error),
    #[error("execution contract failed")]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error("workdir escapes workspace")]
    WorkspaceEscape,
}
