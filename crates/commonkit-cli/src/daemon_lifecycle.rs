use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const LABEL: &str = "com.unsoldgroup.commonkitd";
const PROCESS_EXEC_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_EXEC_POLL_INTERVAL: Duration = Duration::from_millis(10);

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

/// A fixed, machine-readable task-state probe. PowerShell's numeric
/// `ScheduledTaskState` value 4 means Running; no localized text is parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsTaskStatusProbe {
    pub program: &'static str,
    pub arguments: Vec<&'static str>,
}

pub fn windows_task_status_probe() -> WindowsTaskStatusProbe {
    WindowsTaskStatusProbe {
        program: "powershell.exe",
        arguments: vec![
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; $task=Get-ScheduledTask -TaskName 'com.unsoldgroup.commonkitd'; if ([int]$task.State -eq 4) { exit 0 } else { exit 3 }",
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcessIdentity {
    pid: u32,
    executable: PathBuf,
    start_token: String,
    service_root: PathBuf,
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
        } else if backend == DaemonBackend::ProcessFallback {
            home()?.join(".config/commonkit/service")
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
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Principals><Principal id=\"Author\"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure></Settings><Actions Context=\"Author\"><Exec><Command>{executable}</Command></Exec></Actions></Task>"
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
        if self.backend == DaemonBackend::Launchd && self.is_running()? {
            self.launchctl(&["bootout", &format!("{}/{LABEL}", launch_domain()?)])?;
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
                run("systemctl", &["--user", "disable", "commonkitd.service"])?;
            }
            DaemonBackend::WindowsTask => {
                run("schtasks.exe", &["/Delete", "/TN", LABEL, "/F"])?;
            }
            DaemonBackend::ProcessFallback => {}
        }
        remove_if_file(&self.definition_path())?;
        remove_if_file(&self.pid_path())?;
        if self.backend == DaemonBackend::SystemdUser {
            run("systemctl", &["--user", "daemon-reload"])?;
        }
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
            DaemonBackend::WindowsTask => Ok(windows_task_running()),
            DaemonBackend::ProcessFallback => match read_process_identity(&self.pid_path()) {
                Ok(expected) => self.process_identity_matches(&expected),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error.into()),
            },
        }
    }

    fn start_fallback(&self) -> Result<(), DaemonLifecycleError> {
        if self.is_running()? {
            return Ok(());
        }
        remove_if_file(&self.pid_path())?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("commonkitd.log"))?;
        let mut child = Command::new(&self.executable)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?;
        let expected_executable = canonical_path(&self.executable)?;
        let identity = match wait_for_started_identity(
            child.id(),
            &self.root,
            &expected_executable,
            &mut child,
        ) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if !same_executable(&identity.executable, &expected_executable)? {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DaemonLifecycleError::ProcessIdentityMismatch);
        }
        let bytes = serde_json::to_vec(&identity)
            .map_err(|error| DaemonLifecycleError::InvalidProcessIdentity(error.to_string()))?;
        write_atomic_private(&self.pid_path(), &bytes)
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
                let expected = read_process_identity(&self.pid_path())?;
                if !self.process_identity_matches(&expected)? {
                    return Err(DaemonLifecycleError::StaleProcessIdentity);
                }
                terminate_process(&expected.pid.to_string())?;
                remove_if_file(&self.pid_path())?;
            }
        }
        Ok(())
    }

    fn launchctl(&self, args: &[&str]) -> Result<(), DaemonLifecycleError> {
        run("launchctl", args)
    }
    fn process_identity_matches(
        &self,
        expected: &ProcessIdentity,
    ) -> Result<bool, DaemonLifecycleError> {
        if !same_executable(&expected.executable, &self.executable)?
            || expected.service_root != canonical_path(&self.root)?
        {
            return Ok(false);
        }
        Ok(observe_process_identity(expected.pid, &self.root)?.as_ref() == Some(expected))
    }
    fn pid_path(&self) -> PathBuf {
        self.root.join("commonkitd.pid")
    }
}

fn wait_for_started_identity(
    pid: u32,
    service_root: &Path,
    expected_executable: &Path,
    child: &mut std::process::Child,
) -> Result<ProcessIdentity, DaemonLifecycleError> {
    let deadline = Instant::now() + PROCESS_EXEC_TIMEOUT;
    let mut birth_token = None;
    loop {
        if let Some(identity) = observe_process_identity(pid, service_root)? {
            if started_identity_matches(&identity, expected_executable, &mut birth_token)? {
                return Ok(identity);
            }
        }
        if child.try_wait()?.is_some() {
            return Err(DaemonLifecycleError::ProcessIdentityUnavailable);
        }
        if Instant::now() >= deadline {
            return Err(DaemonLifecycleError::ProcessExecTimeout);
        }
        std::thread::sleep(PROCESS_EXEC_POLL_INTERVAL);
    }
}

fn started_identity_matches(
    identity: &ProcessIdentity,
    expected_executable: &Path,
    birth_token: &mut Option<String>,
) -> Result<bool, DaemonLifecycleError> {
    match birth_token.as_ref() {
        Some(token) if token != &identity.start_token => {
            return Err(DaemonLifecycleError::ProcessIdentityMismatch);
        }
        None => *birth_token = Some(identity.start_token.clone()),
        _ => {}
    }
    Ok(same_executable(&identity.executable, expected_executable)?)
}

fn same_executable(left: &Path, right: &Path) -> Result<bool, io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = fs::metadata(left)?;
        let right = fs::metadata(right)?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(unix))]
    Ok(canonical_path(left)? == canonical_path(right)?)
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
fn canonical_path(path: &Path) -> Result<PathBuf, io::Error> {
    path.canonicalize()
}

fn read_process_identity(path: &Path) -> Result<ProcessIdentity, io::Error> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn observe_process_identity(
    pid: u32,
    service_root: &Path,
) -> Result<Option<ProcessIdentity>, DaemonLifecycleError> {
    let service_root = canonical_path(service_root)?;
    #[cfg(target_os = "linux")]
    {
        let executable = match fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(path) => canonical_path(&path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let close = stat
            .rfind(')')
            .ok_or(DaemonLifecycleError::ProcessIdentityUnavailable)?;
        let fields: Vec<_> = stat[close + 1..].split_whitespace().collect();
        let start_token = fields
            .get(19)
            .ok_or(DaemonLifecycleError::ProcessIdentityUnavailable)?
            .to_string();
        return Ok(Some(ProcessIdentity {
            pid,
            executable,
            start_token,
            service_root,
        }));
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        use std::mem::{MaybeUninit, size_of};
        use std::os::unix::ffi::OsStrExt;

        let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        // SAFETY: `info` points to a correctly sized writable proc_bsdinfo and
        // is only assumed initialized when libproc reports the complete size.
        let info_size = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size_of::<libc::proc_bsdinfo>() as i32,
            )
        };
        if info_size == 0 {
            return Ok(None);
        }
        if info_size != size_of::<libc::proc_bsdinfo>() as i32 {
            return Err(DaemonLifecycleError::ProcessIdentityUnavailable);
        }
        // SAFETY: the exact-size check above proves libproc initialized info.
        let info = unsafe { info.assume_init() };

        let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize + 1];
        // SAFETY: `path` is a writable buffer with the supplied byte length.
        let path_size = unsafe {
            libc::proc_pidpath(
                pid as i32,
                path.as_mut_ptr().cast(),
                libc::PROC_PIDPATHINFO_MAXSIZE as u32,
            )
        };
        if path_size <= 0 {
            return Err(DaemonLifecycleError::ProcessIdentityUnavailable);
        }
        path[path_size as usize] = 0;
        // SAFETY: proc_pidpath returned `path_size` initialized bytes and the
        // next byte was set to NUL above.
        let executable = unsafe { CStr::from_ptr(path.as_ptr().cast()) };
        return Ok(Some(ProcessIdentity {
            pid,
            executable: canonical_path(Path::new(std::ffi::OsStr::from_bytes(
                executable.to_bytes(),
            )))?,
            start_token: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
            service_root,
        }));
    }
    #[cfg(windows)]
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct WindowsProcess {
            executable_path: PathBuf,
            creation_date: String,
        }
        let script = format!(
            "$p=Get-CimInstance Win32_Process -Filter 'ProcessId={pid}'; if ($null -eq $p) {{ exit 3 }}; $p | Select-Object ExecutablePath,CreationDate | ConvertTo-Json -Compress"
        );
        let output = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .stdin(Stdio::null())
            .output()?;
        if output.status.code() == Some(3) {
            return Ok(None);
        }
        if !output.status.success() {
            return Err(DaemonLifecycleError::CommandFailed("powershell.exe".into()));
        }
        let observed: WindowsProcess = serde_json::from_slice(&output.stdout)
            .map_err(|error| DaemonLifecycleError::InvalidProcessIdentity(error.to_string()))?;
        return Ok(Some(ProcessIdentity {
            pid,
            executable: canonical_path(&observed.executable_path)?,
            start_token: observed.creation_date,
            service_root,
        }));
    }
    #[allow(unreachable_code)]
    Ok(None)
}

fn windows_task_running() -> bool {
    let probe = windows_task_status_probe();
    Command::new(probe.program)
        .args(probe.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| DaemonLifecycleError::ClockInvalid)?
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
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
        #[cfg(windows)]
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
    #[error("stored daemon PID no longer identifies the configured executable")]
    StaleProcessIdentity,
    #[error("the daemon process identity could not be observed")]
    ProcessIdentityUnavailable,
    #[error("the started daemon executable does not match the configured executable")]
    ProcessIdentityMismatch,
    #[error(
        "the daemon did not transition to the configured executable before the safety timeout; verify commonkitd is a directly executable binary, not a script or wrapper"
    )]
    ProcessExecTimeout,
    #[error("stored daemon process identity is invalid: {0}")]
    InvalidProcessIdentity(String),
    #[error("system clock is before the Unix epoch")]
    ClockInvalid,
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn hosted_spawn_transition_waits_for_exec_while_binding_the_pid_birth_token() {
        let expected = if Path::new("/usr/bin/yes").is_file() {
            PathBuf::from("/usr/bin/yes")
        } else {
            PathBuf::from("/bin/yes")
        };
        let pre_exec = ProcessIdentity {
            pid: 42,
            executable: std::env::current_exe().unwrap(),
            start_token: "same-linux-proc-start-time".into(),
            service_root: PathBuf::from("/tmp/commonkit-service"),
        };
        let post_exec = ProcessIdentity {
            executable: expected.clone(),
            ..pre_exec.clone()
        };
        let mut birth_token = None;

        assert!(!started_identity_matches(&pre_exec, &expected, &mut birth_token).unwrap());
        assert!(started_identity_matches(&post_exec, &expected, &mut birth_token).unwrap());

        let replacement = ProcessIdentity {
            start_token: "reused-pid-start-time".into(),
            ..post_exec
        };
        assert!(matches!(
            started_identity_matches(&replacement, &expected, &mut birth_token),
            Err(DaemonLifecycleError::ProcessIdentityMismatch)
        ));
    }
}
