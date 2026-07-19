use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use thiserror::Error;

const LABEL: &str = "com.unsoldgroup.commonkitd";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonBackend {
    Launchd,
    SystemdUser,
    WindowsTask,
    ProcessFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonServiceStatus {
    pub backend: DaemonBackend,
    pub installed: bool,
    pub running: bool,
    pub definition: PathBuf,
}

pub struct DaemonService {
    backend: DaemonBackend,
    root: PathBuf,
    executable: PathBuf,
}

impl DaemonService {
    pub fn configured(
        backend: DaemonBackend,
        root: PathBuf,
        executable: PathBuf,
    ) -> Result<Self, DaemonLifecycleError> {
        if !root.is_absolute() || !executable.is_absolute() {
            return Err(DaemonLifecycleError::ExecutableMustBeAbsolute);
        }
        Ok(Self {
            backend,
            root,
            executable,
        })
    }

    pub fn discover(executable: PathBuf) -> Result<Self, DaemonLifecycleError> {
        if !executable.is_absolute() {
            return Err(DaemonLifecycleError::ExecutableMustBeAbsolute);
        }
        let metadata = fs::symlink_metadata(&executable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DaemonLifecycleError::UnsafeExecutable);
        }
        let backend = match std::env::var("COMMONKIT_SERVICE_BACKEND").ok().as_deref() {
            Some("process_fallback") => DaemonBackend::ProcessFallback,
            Some(other) => return Err(DaemonLifecycleError::UnsupportedBackend(other.into())),
            None if cfg!(target_os = "macos") => DaemonBackend::Launchd,
            None if cfg!(target_os = "windows") => DaemonBackend::WindowsTask,
            None => {
                let available = Command::new("systemctl")
                    .args(["--user", "show-environment"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                if available {
                    DaemonBackend::SystemdUser
                } else {
                    DaemonBackend::ProcessFallback
                }
            }
        };
        let root = if let Some(root) = std::env::var_os("COMMONKIT_SERVICE_ROOT") {
            PathBuf::from(root)
        } else if cfg!(target_os = "macos") {
            home()?.join("Library/LaunchAgents")
        } else if cfg!(target_os = "windows") {
            home()?.join("AppData/Local/CommonKit/service")
        } else {
            home()?.join(".config/systemd/user")
        };
        Self::configured(backend, root, executable)
    }

    pub fn definition_path(&self) -> PathBuf {
        match self.backend {
            DaemonBackend::Launchd => self.root.join(format!("{LABEL}.plist")),
            DaemonBackend::SystemdUser => self.root.join("commonkitd.service"),
            DaemonBackend::WindowsTask => self.root.join("commonkitd-task.xml"),
            DaemonBackend::ProcessFallback => self.root.join("commonkitd.json"),
        }
    }

    pub fn render_definition(&self) -> String {
        let executable = escape_xml(&self.executable.to_string_lossy());
        match self.backend {
            DaemonBackend::Launchd => format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{executable}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n"
            ),
            DaemonBackend::SystemdUser => format!(
                "[Unit]\nDescription=CommonKit control daemon\n\n[Service]\nType=simple\nExecStart={}\nRestart=on-failure\nNoNewPrivileges=true\n\n[Install]\nWantedBy=default.target\n",
                escape_systemd(&self.executable.to_string_lossy())
            ),
            DaemonBackend::WindowsTask => format!(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Principals><Principal id=\"Author\"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure></Settings><Actions Context=\"Author\"><Exec><Command>{executable}</Command></Exec></Actions></Task>"
            ),
            DaemonBackend::ProcessFallback => format!(
                "{{\"executable\":{}}}\n",
                serde_json::to_string(&self.executable).expect("path serializes")
            ),
        }
    }

    pub fn install(&self) -> Result<DaemonServiceStatus, DaemonLifecycleError> {
        fs::create_dir_all(&self.root)?;
        let root_metadata = fs::symlink_metadata(&self.root)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(DaemonLifecycleError::UnsafeServiceRoot);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        write_atomic_private(&self.definition_path(), self.render_definition().as_bytes())?;
        match self.backend {
            DaemonBackend::Launchd => self.launchctl(&[
                "bootstrap",
                &launch_domain()?,
                path_str(&self.definition_path())?,
            ])?,
            DaemonBackend::SystemdUser => {
                run("systemctl", &["--user", "daemon-reload"])?;
                run("systemctl", &["--user", "enable", "commonkitd.service"])?;
            }
            DaemonBackend::WindowsTask => run(
                "schtasks.exe",
                &[
                    "/Create",
                    "/TN",
                    LABEL,
                    "/XML",
                    path_str(&self.definition_path())?,
                    "/F",
                ],
            )?,
            DaemonBackend::ProcessFallback => {}
        }
        self.status()
    }

    pub fn start(&self) -> Result<DaemonServiceStatus, DaemonLifecycleError> {
        if !self.definition_path().is_file() {
            return Err(DaemonLifecycleError::NotInstalled);
        }
        match self.backend {
            DaemonBackend::Launchd => {
                self.launchctl(&["kickstart", "-k", &format!("{}/{LABEL}", launch_domain()?)])?
            }
            DaemonBackend::SystemdUser => {
                run("systemctl", &["--user", "start", "commonkitd.service"])?
            }
            DaemonBackend::WindowsTask => run("schtasks.exe", &["/Run", "/TN", LABEL])?,
            DaemonBackend::ProcessFallback => self.start_fallback()?,
        }
        self.status()
    }

    pub fn restart(&self) -> Result<DaemonServiceStatus, DaemonLifecycleError> {
        self.stop_if_running()?;
        self.start()
    }

    pub fn uninstall(&self) -> Result<DaemonServiceStatus, DaemonLifecycleError> {
        self.stop_if_running()?;
        match self.backend {
            DaemonBackend::Launchd => {
                // bootout is performed by stop; absence is already tolerated there.
            }
            DaemonBackend::SystemdUser => {
                let _ = run("systemctl", &["--user", "disable", "commonkitd.service"]);
                let _ = run("systemctl", &["--user", "daemon-reload"]);
            }
            DaemonBackend::WindowsTask => {
                let _ = run("schtasks.exe", &["/Delete", "/TN", LABEL, "/F"]);
            }
            DaemonBackend::ProcessFallback => {}
        }
        remove_if_file(&self.definition_path())?;
        remove_if_file(&self.pid_path())?;
        self.status()
    }

    pub fn status(&self) -> Result<DaemonServiceStatus, DaemonLifecycleError> {
        let installed = self.definition_path().is_file();
        let running = if !installed {
            false
        } else {
            self.is_running()?
        };
        Ok(DaemonServiceStatus {
            backend: self.backend,
            installed,
            running,
            definition: self.definition_path(),
        })
    }

    fn is_running(&self) -> Result<bool, DaemonLifecycleError> {
        match self.backend {
            DaemonBackend::Launchd => Ok(run_status(
                "launchctl",
                &["print", &format!("{}/{LABEL}", launch_domain()?)],
            )),
            DaemonBackend::SystemdUser => Ok(run_status(
                "systemctl",
                &["--user", "is-active", "--quiet", "commonkitd.service"],
            )),
            DaemonBackend::WindowsTask => Ok(run_status(
                "schtasks.exe",
                &["/Query", "/TN", LABEL, "/FO", "LIST"],
            )),
            DaemonBackend::ProcessFallback => match fs::read_to_string(self.pid_path()) {
                Ok(pid) => Ok(process_alive(pid.trim())),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error.into()),
            },
        }
    }

    fn start_fallback(&self) -> Result<(), DaemonLifecycleError> {
        if self.is_running()? {
            return Ok(());
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("commonkitd.log"))?;
        let child = Command::new(&self.executable)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?;
        write_atomic_private(&self.pid_path(), child.id().to_string().as_bytes())
    }

    fn stop_if_running(&self) -> Result<(), DaemonLifecycleError> {
        if !self.is_running()? {
            return Ok(());
        }
        match self.backend {
            DaemonBackend::Launchd => {
                self.launchctl(&["bootout", &format!("{}/{LABEL}", launch_domain()?)])?
            }
            DaemonBackend::SystemdUser => {
                run("systemctl", &["--user", "stop", "commonkitd.service"])?
            }
            DaemonBackend::WindowsTask => run("schtasks.exe", &["/End", "/TN", LABEL])?,
            DaemonBackend::ProcessFallback => {
                let pid = fs::read_to_string(self.pid_path())?;
                terminate_process(pid.trim())?;
                remove_if_file(&self.pid_path())?;
            }
        }
        Ok(())
    }

    fn launchctl(&self, args: &[&str]) -> Result<(), DaemonLifecycleError> {
        run("launchctl", args)
    }
    fn pid_path(&self) -> PathBuf {
        self.root.join("commonkitd.pid")
    }
}

fn home() -> Result<PathBuf, DaemonLifecycleError> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .ok_or(DaemonLifecycleError::HomeMissing)
}
fn path_str(path: &Path) -> Result<&str, DaemonLifecycleError> {
    path.to_str().ok_or(DaemonLifecycleError::NonUtf8Path)
}
fn launch_domain() -> Result<String, DaemonLifecycleError> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err(DaemonLifecycleError::CommandFailed("id".into()));
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}
fn run(program: &str, args: &[&str]) -> Result<(), DaemonLifecycleError> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(DaemonLifecycleError::CommandFailed(program.into()))
    }
}
fn run_status(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
fn process_alive(pid: &str) -> bool {
    if cfg!(windows) {
        run_status("tasklist.exe", &["/FI", &format!("PID eq {pid}"), "/NH"])
    } else {
        run_status("kill", &["-0", pid])
    }
}
fn terminate_process(pid: &str) -> Result<(), DaemonLifecycleError> {
    if cfg!(windows) {
        run("taskkill.exe", &["/PID", pid, "/T", "/F"])
    } else {
        run("kill", &["-TERM", pid])
    }
}
fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<(), DaemonLifecycleError> {
    use std::io::Write;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            let _ = fs::remove_file(&temporary);
            return Err(DaemonLifecycleError::UnsafeDefinition);
        }
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}
fn remove_if_file(path: &Path) -> Result<(), io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn escape_systemd(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Debug, Error)]
pub enum DaemonLifecycleError {
    #[error("daemon executable path must be absolute")]
    ExecutableMustBeAbsolute,
    #[error("daemon executable must be a regular non-symlink file")]
    UnsafeExecutable,
    #[error("service root must be a regular non-symlink directory")]
    UnsafeServiceRoot,
    #[error("service definition must be a regular non-symlink file")]
    UnsafeDefinition,
    #[error("home directory is unavailable")]
    HomeMissing,
    #[error("service path is not UTF-8")]
    NonUtf8Path,
    #[error("unsupported service backend override: {0}")]
    UnsupportedBackend(String),
    #[error("daemon service is not installed")]
    NotInstalled,
    #[error("service command failed: {0}")]
    CommandFailed(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}
