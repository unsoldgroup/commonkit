use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Output;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::time::Duration;

use super::provider::ProviderFailure;

const PROVIDER_RUNTIME_LIMIT: Duration = Duration::from_secs(300);
const PROVIDER_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
struct ProviderLimits {
    runtime: Duration,
    output_bytes: usize,
}

pub(crate) struct ProviderSandbox<'a> {
    executable: &'a Path,
    args: Vec<OsString>,
    environment: BTreeMap<OsString, OsString>,
    current_dir: &'a Path,
    writable_roots: Vec<PathBuf>,
    readable_paths: Vec<PathBuf>,
    limits: ProviderLimits,
}

impl<'a> ProviderSandbox<'a> {
    pub(crate) fn new(executable: &'a Path, current_dir: &'a Path) -> Self {
        Self {
            executable,
            args: Vec::new(),
            environment: BTreeMap::new(),
            current_dir,
            writable_roots: Vec::new(),
            readable_paths: vec![executable.to_path_buf()],
            limits: ProviderLimits {
                runtime: PROVIDER_RUNTIME_LIMIT,
                output_bytes: PROVIDER_OUTPUT_LIMIT,
            },
        }
    }

    pub(crate) fn arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(value.as_ref().to_owned());
        self
    }

    pub(crate) fn args<I, S>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(values.into_iter().map(|value| value.as_ref().to_owned()));
        self
    }

    pub(crate) fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.environment
            .insert(key.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }

    pub(crate) fn writable_root(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.writable_roots.push(path.as_ref().to_path_buf());
        self
    }

    pub(crate) fn readable_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.readable_paths.push(path.as_ref().to_path_buf());
        self
    }

    pub(crate) fn limits(&mut self, runtime: Duration, output_bytes: usize) -> &mut Self {
        self.limits = ProviderLimits {
            runtime,
            output_bytes,
        };
        self
    }

    pub(crate) fn output(&self) -> Result<Output, ProviderFailure> {
        platform_output(self)
    }

    #[cfg(all(test, any(target_os = "linux", windows)))]
    fn test_limits(&mut self, runtime: Duration, output_bytes: usize) -> &mut Self {
        self.limits = ProviderLimits {
            runtime,
            output_bytes,
        };
        self
    }
}

#[cfg(not(windows))]
fn unavailable(platform: &str, remediation: &str) -> ProviderFailure {
    ProviderFailure::Materialize(format!(
        "provider sandbox capability is unavailable on {platform}; {remediation}; provider execution was denied before launch"
    ))
}

#[cfg(target_os = "macos")]
fn platform_output(spec: &ProviderSandbox<'_>) -> Result<Output, ProviderFailure> {
    let _ = (
        spec.executable,
        &spec.args,
        &spec.environment,
        spec.current_dir,
        &spec.writable_roots,
        &spec.readable_paths,
        spec.limits.runtime,
        spec.limits.output_bytes,
    );
    Err(unavailable(
        "macOS",
        "external providers require a privileged disposable-user helper or VM/container boundary that can terminate detached descendants; configure a native provider fallback until that isolation helper is installed",
    ))
}

#[cfg(target_os = "linux")]
fn platform_output(spec: &ProviderSandbox<'_>) -> Result<Output, ProviderFailure> {
    let launcher = Path::new("/usr/bin/bwrap");
    if !launcher.is_file() {
        return Err(unavailable(
            "Linux",
            "install the distribution's bubblewrap package at /usr/bin/bwrap",
        ));
    }
    let mut command = Command::new(launcher);
    command.args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--clearenv",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]);
    for runtime in ["/usr", "/bin", "/lib", "/lib64"] {
        let path = Path::new(runtime);
        if path.exists() {
            command.arg("--ro-bind").arg(path).arg(path);
        }
    }
    let mut paths = spec.readable_paths.clone();
    paths.extend(spec.writable_roots.iter().cloned());
    paths.push(spec.current_dir.to_path_buf());
    ensure_sandbox_parents(&mut command, &paths);
    for path in &spec.readable_paths {
        command.arg("--ro-bind").arg(path).arg(path);
    }
    for path in &spec.writable_roots {
        command.arg("--bind").arg(path).arg(path);
    }
    command.arg("--chdir").arg(spec.current_dir);
    for (key, value) in &spec.environment {
        command.arg("--setenv").arg(key).arg(value);
    }
    command.arg("--").arg(spec.executable).args(&spec.args);
    bounded_unix_output(command, spec.limits, "Linux")
}

#[cfg(target_os = "linux")]
fn bounded_unix_output(
    mut command: Command,
    limits: ProviderLimits,
    platform: &str,
) -> Result<Output, ProviderFailure> {
    use std::os::unix::process::CommandExt as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(|error| {
        ProviderFailure::Materialize(format!(
            "could not launch {platform} provider sandbox: {error}"
        ))
    })?;
    let process_group = child.id() as i32;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_reader = bounded_reader(
        child.stdout.take().expect("piped provider stdout"),
        limits.output_bytes,
        Arc::clone(&overflow),
    );
    let stderr_reader = bounded_reader(
        child.stderr.take().expect("piped provider stderr"),
        limits.output_bytes,
        Arc::clone(&overflow),
    );
    let started = Instant::now();
    let mut limit_failure = None;
    let status = loop {
        if overflow.load(Ordering::Acquire) {
            limit_failure = Some(format!(
                "provider output limit of {} bytes per stream was exceeded",
                limits.output_bytes
            ));
            terminate_unix_group(process_group, &mut child);
            break child.wait();
        }
        if started.elapsed() >= limits.runtime {
            limit_failure = Some(format!(
                "provider runtime limit of {} seconds was exceeded",
                limits.runtime.as_secs_f64()
            ));
            terminate_unix_group(process_group, &mut child);
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // Provider completion never licenses detached descendants.
                terminate_unix_process_group(process_group);
                break Ok(status);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => break Err(error),
        }
    }
    .map_err(|error| {
        ProviderFailure::Materialize(format!(
            "could not wait for {platform} provider sandbox: {error}"
        ))
    })?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| ProviderFailure::Materialize("provider stdout reader panicked".into()))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ProviderFailure::Materialize("provider stderr reader panicked".into()))??;
    if overflow.load(Ordering::Acquire) && limit_failure.is_none() {
        limit_failure = Some(format!(
            "provider output limit of {} bytes per stream was exceeded",
            limits.output_bytes
        ));
    }
    if let Some(reason) = limit_failure {
        return Err(ProviderFailure::Materialize(reason));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(target_os = "linux")]
fn bounded_reader<R: std::io::Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<Result<Vec<u8>, ProviderFailure>> {
    use std::sync::atomic::Ordering;

    std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(limit.min(8192));
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buffer).map_err(|error| {
                ProviderFailure::Materialize(format!("could not capture provider output: {error}"))
            })?;
            if read == 0 {
                return Ok(bytes);
            }
            if bytes.len().saturating_add(read) > limit {
                let remaining = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..remaining]);
                overflow.store(true, Ordering::Release);
                return Ok(bytes);
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
    })
}

#[cfg(target_os = "linux")]
fn terminate_unix_group(process_group: i32, child: &mut std::process::Child) {
    terminate_unix_process_group(process_group);
    let _ = child.kill();
}

#[cfg(target_os = "linux")]
fn terminate_unix_process_group(process_group: i32) {
    unsafe {
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(target_os = "linux")]
fn ensure_sandbox_parents(command: &mut Command, paths: &[PathBuf]) {
    let mut parents = std::collections::BTreeSet::new();
    for path in paths {
        let mut parent = path.parent();
        while let Some(value) = parent {
            if value != Path::new("/")
                && !["/usr", "/bin", "/lib", "/lib64"].contains(&value.to_str().unwrap_or(""))
            {
                parents.insert(value.to_path_buf());
            }
            parent = value.parent();
        }
    }
    for parent in parents {
        command.arg("--dir").arg(parent);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod unix_limit_tests {
    use super::ProviderSandbox;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().expect("root");
        let executable = root.path().join("provider");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");
        std::fs::write(&executable, script).expect("script");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("permissions");
        (root, executable, workspace)
    }

    #[test]
    fn provider_runtime_limit_terminates_the_process() {
        let (_root, executable, workspace) = fixture("#!/bin/sh\nsleep 30\n");
        let mut command = ProviderSandbox::new(&executable, &workspace);
        command
            .writable_root(&workspace)
            .test_limits(Duration::from_millis(100), 1024);

        let error = command
            .output()
            .expect_err("provider must time out")
            .to_string();

        assert!(error.contains("runtime limit"), "{error}");
    }

    #[test]
    fn provider_output_limit_terminates_unbounded_output() {
        let (_root, executable, workspace) =
            fixture("#!/bin/sh\nwhile :; do printf '0123456789abcdef'; done\n");
        let mut command = ProviderSandbox::new(&executable, &workspace);
        command
            .writable_root(&workspace)
            .test_limits(Duration::from_secs(2), 1024);

        let error = command
            .output()
            .expect_err("provider output must be bounded")
            .to_string();

        assert!(error.contains("output limit"), "{error}");
    }

    #[test]
    fn successful_provider_cannot_leave_a_descendant_running() {
        let (_root, executable, workspace) = fixture(
            "#!/bin/sh\n(sleep 1; printf survived > \"$PWD/descendant-marker\") &\nexit 0\n",
        );
        let mut command = ProviderSandbox::new(&executable, &workspace);
        command
            .writable_root(&workspace)
            .test_limits(Duration::from_secs(2), 1024);

        let output = command.output().expect("provider launch");
        assert!(output.status.success(), "{output:?}");
        std::thread::sleep(Duration::from_millis(1_200));
        assert!(
            !workspace.join("descendant-marker").exists(),
            "provider descendant survived sandbox completion"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_fail_closed_tests {
    use super::ProviderSandbox;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn detached_descendant_risk_fails_closed_before_provider_launch() {
        let root = tempfile::tempdir().expect("root");
        let executable = root.path().join("provider");
        let workspace = root.path().join("workspace");
        let marker = root.path().join("executed");
        std::fs::create_dir(&workspace).expect("workspace");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
        )
        .expect("provider");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("permissions");
        let mut command = ProviderSandbox::new(&executable, &workspace);
        command.writable_root(&workspace);

        let error = command
            .output()
            .expect_err("macOS provider must fail closed")
            .to_string();

        assert!(
            error.contains("privileged disposable-user helper"),
            "{error}"
        );
        assert!(error.contains("VM/container boundary"), "{error}");
        assert!(
            !marker.exists(),
            "provider launched before isolation failed"
        );
    }
}

#[cfg(windows)]
fn platform_output(spec: &ProviderSandbox<'_>) -> Result<Output, ProviderFailure> {
    windows_appcontainer::output(spec)
}

#[cfg(windows)]
mod windows_appcontainer {
    use super::{ProviderFailure, ProviderSandbox};
    use sha2::{Digest, Sha256};
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs::File;
    use std::io::Read;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::{ExitStatus, Output};
    use std::ptr::{null, null_mut};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, LocalFree,
        SetHandleInformation, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::{
        BuildTrusteeWithSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
        SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, FreeSid, NO_INHERITANCE, PSID, SECURITY_ATTRIBUTES,
        SECURITY_CAPABILITIES,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
        InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
        WaitForSingleObject,
    };

    pub(super) fn output(spec: &ProviderSandbox<'_>) -> Result<Output, ProviderFailure> {
        let executable = canonical_existing(spec.executable, "executable")?;
        let current_dir = canonical_existing(spec.current_dir, "working directory")?;
        let sid = AppContainerSid::derive(&sandbox_name(&executable, &current_dir))?;

        // ACL guards deliberately outlive the child. Their Drop implementations restore
        // the exact previous DACL even if any later launch step fails.
        let mut permissions = std::collections::BTreeMap::new();
        add_permission(
            &mut permissions,
            executable.clone(),
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        );
        add_permission(
            &mut permissions,
            current_dir.clone(),
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        );
        for path in &spec.readable_paths {
            let path = canonical_existing(path, "read-only provider input")?;
            for entry in existing_tree(&path)? {
                add_permission(
                    &mut permissions,
                    entry,
                    FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                );
            }
        }
        for path in &spec.writable_roots {
            let path = canonical_existing(path, "writable staging root")?;
            if !path.is_dir() {
                return Err(failure(format!(
                    "writable staging root is not a directory: {}",
                    path.display()
                )));
            }
            for entry in existing_tree(&path)? {
                add_permission(
                    &mut permissions,
                    entry.clone(),
                    writable_permissions(&entry),
                );
            }
        }
        // Non-inheriting grants are intentional. Existing trees are enumerated exactly;
        // new staging entries are owned by the AppContainer that creates them. This avoids
        // leaving inherited sandbox ACEs behind after the parent DACL is restored.
        let mut grants = Vec::with_capacity(permissions.len());
        for (path, permission) in permissions {
            grants.push(AclGrant::install(&path, sid.0, permission)?);
        }

        let stdin_pipe = InputPipe::new()?;
        let stdout_pipe = Pipe::new()?;
        let stderr_pipe = Pipe::new()?;
        let attributes = AttributeList::security_capabilities(
            sid.0,
            &[stdin_pipe.read.0, stdout_pipe.write.0, stderr_pipe.write.0],
        )?;
        let mut job = Some(Job::kill_on_close()?);

        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdin_pipe.read.0;
        startup.StartupInfo.hStdOutput = stdout_pipe.write.0;
        startup.StartupInfo.hStdError = stderr_pipe.write.0;
        startup.lpAttributeList = attributes.ptr;

        let mut command_line = command_line(&executable, &spec.args);
        let application = wide_nul(executable.as_os_str());
        let directory = wide_nul(current_dir.as_os_str());
        let environment = environment_block(&spec.environment);
        let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                (&startup as *const STARTUPINFOEXW).cast(),
                &mut process,
            )
        };
        if created == 0 {
            return Err(last_error(
                "could not create Windows AppContainer provider process",
            ));
        }
        drop(stdin_pipe);
        let process_handle = OwnedHandle(process.hProcess);
        let thread_handle = OwnedHandle(process.hThread);
        if unsafe {
            AssignProcessToJobObject(job.as_ref().expect("provider job").0.0, process_handle.0)
        } == 0
        {
            unsafe { TerminateProcess(process_handle.0, 1) };
            return Err(last_error(
                "could not attach provider process to containment job",
            ));
        }

        // The parent must release its writer copies before readers can observe EOF.
        drop(stdout_pipe.write);
        drop(stderr_pipe.write);
        let overflow = Arc::new(AtomicBool::new(false));
        let stdout_reader = reader(
            stdout_pipe.read,
            spec.limits.output_bytes,
            Arc::clone(&overflow),
        );
        let stderr_reader = reader(
            stderr_pipe.read,
            spec.limits.output_bytes,
            Arc::clone(&overflow),
        );
        if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
            unsafe { TerminateProcess(process_handle.0, 1) };
            return Err(last_error("could not resume contained provider process"));
        }
        drop(thread_handle);
        let started = Instant::now();
        let mut limit_failure = None;
        loop {
            match unsafe { WaitForSingleObject(process_handle.0, 10) } {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT => {
                    if overflow.load(Ordering::Acquire) {
                        limit_failure = Some(format!(
                            "provider output limit of {} bytes per stream was exceeded",
                            spec.limits.output_bytes
                        ));
                    } else if started.elapsed() >= spec.limits.runtime {
                        limit_failure = Some(format!(
                            "provider runtime limit of {} seconds was exceeded",
                            spec.limits.runtime.as_secs_f64()
                        ));
                    }
                    if limit_failure.is_some() {
                        // Job close terminates the provider and every descendant.
                        drop(job.take());
                        if unsafe { WaitForSingleObject(process_handle.0, INFINITE) } == WAIT_FAILED
                        {
                            return Err(last_error(
                                "could not wait for terminated provider process",
                            ));
                        }
                        break;
                    }
                }
                WAIT_FAILED => {
                    return Err(last_error("could not wait for contained provider process"));
                }
                unexpected => {
                    return Err(failure(format!(
                        "unexpected Windows provider wait result {unexpected}"
                    )));
                }
            }
        }
        let mut exit_code = 0;
        if unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) } == 0 {
            return Err(last_error("could not read contained provider exit status"));
        }
        // Close the job before joining pipe readers. A malicious provider may leave a
        // descendant holding inherited writer handles; kill-on-close guarantees EOF.
        drop(job.take());
        let stdout = stdout_reader
            .join()
            .map_err(|_| failure("provider stdout reader panicked"))?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| failure("provider stderr reader panicked"))?;

        // Explicit drops make the security cleanup order auditable: terminate descendants,
        // release process handles, restore every DACL, then release the derived SID.
        drop(process_handle);
        for grant in grants.iter_mut().rev() {
            grant.restore()?;
        }
        drop(grants);
        drop(sid);
        let stdout = stdout?;
        let stderr = stderr?;
        if overflow.load(Ordering::Acquire) && limit_failure.is_none() {
            limit_failure = Some(format!(
                "provider output limit of {} bytes per stream was exceeded",
                spec.limits.output_bytes
            ));
        }
        if let Some(reason) = limit_failure {
            return Err(failure(reason));
        }
        use std::os::windows::process::ExitStatusExt;
        Ok(Output {
            status: ExitStatus::from_raw(exit_code),
            stdout,
            stderr,
        })
    }

    fn failure(message: impl Into<String>) -> ProviderFailure {
        ProviderFailure::Materialize(message.into())
    }

    fn last_error(context: &str) -> ProviderFailure {
        let code = unsafe { GetLastError() };
        failure(format!("{context} (Windows error {code})"))
    }

    fn canonical_existing(path: &Path, kind: &str) -> Result<PathBuf, ProviderFailure> {
        path.canonicalize().map_err(|error| {
            failure(format!(
                "provider sandbox could not canonicalize {kind} {}: {error}",
                path.display()
            ))
        })
    }

    fn add_permission(
        permissions: &mut std::collections::BTreeMap<PathBuf, u32>,
        path: PathBuf,
        permission: u32,
    ) {
        permissions
            .entry(path)
            .and_modify(|existing| *existing |= permission)
            .or_insert(permission);
    }

    pub(super) fn writable_permissions(path: &Path) -> u32 {
        let mut permissions =
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;
        if path.is_dir() {
            permissions |= FILE_DELETE_CHILD;
        }
        permissions
    }

    fn existing_tree(root: &Path) -> Result<Vec<PathBuf>, ProviderFailure> {
        let mut entries = vec![root.to_path_buf()];
        if !root.is_dir() {
            return Ok(entries);
        }
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let children = std::fs::read_dir(&directory).map_err(|error| {
                failure(format!(
                    "provider sandbox could not enumerate {}: {error}",
                    directory.display()
                ))
            })?;
            for child in children {
                let child = child.map_err(|error| {
                    failure(format!(
                        "provider sandbox could not enumerate {}: {error}",
                        directory.display()
                    ))
                })?;
                let path = child.path();
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                    failure(format!(
                        "provider sandbox could not inspect {}: {error}",
                        path.display()
                    ))
                })?;
                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(failure(format!(
                        "provider sandbox denied reparse-point escape candidate {}",
                        path.display()
                    )));
                }
                entries.push(path.clone());
                if metadata.is_dir() {
                    pending.push(path);
                }
            }
        }
        entries.sort();
        Ok(entries)
    }

    fn sandbox_name(executable: &Path, staging: &Path) -> Vec<u16> {
        let mut digest = Sha256::new();
        digest.update(executable.as_os_str().to_string_lossy().to_lowercase());
        digest.update([0]);
        digest.update(staging.as_os_str().to_string_lossy().to_lowercase());
        let digest = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        wide_nul(OsStr::new(&format!("CommonKit.Provider.{}", &digest[..24])))
    }

    fn wide_nul(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain([0]).collect()
    }

    fn quote_arg(value: &OsStr) -> Vec<u16> {
        let input: Vec<u16> = value.encode_wide().collect();
        if !input.is_empty()
            && !input.iter().any(|unit| {
                *unit == u16::from(b' ') || *unit == u16::from(b'\t') || *unit == u16::from(b'"')
            })
        {
            return input;
        }
        let mut quoted = vec![b'"' as u16];
        let mut slashes = 0;
        for unit in input {
            if unit == b'\\' as u16 {
                slashes += 1;
            } else if unit == b'"' as u16 {
                quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
                quoted.push(unit);
                slashes = 0;
            } else {
                quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes));
                quoted.push(unit);
                slashes = 0;
            }
        }
        quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        quoted.push(b'"' as u16);
        quoted
    }

    fn command_line(executable: &Path, args: &[OsString]) -> Vec<u16> {
        let mut result = quote_arg(executable.as_os_str());
        for arg in args {
            result.push(b' ' as u16);
            result.extend(quote_arg(arg));
        }
        result.push(0);
        result
    }

    fn environment_block(environment: &std::collections::BTreeMap<OsString, OsString>) -> Vec<u16> {
        let mut block = Vec::new();
        for (key, value) in environment {
            block.extend(key.encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        block
    }

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct AppContainerSid(PSID);
    impl AppContainerSid {
        fn derive(name: &[u16]) -> Result<Self, ProviderFailure> {
            let mut sid = null_mut();
            let result =
                unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            if result < 0 || sid.is_null() {
                return Err(failure(format!(
                    "could not derive ephemeral Windows AppContainer SID (HRESULT {result:#x})"
                )));
            }
            Ok(Self(sid))
        }
    }
    impl Drop for AppContainerSid {
        fn drop(&mut self) {
            unsafe { FreeSid(self.0) };
        }
    }

    struct AclGrant {
        path: Vec<u16>,
        old_dacl: *mut windows_sys::Win32::Security::ACL,
        security_descriptor: *mut c_void,
        restored: bool,
    }
    impl AclGrant {
        fn install(path: &Path, sid: PSID, permissions: u32) -> Result<Self, ProviderFailure> {
            let path = wide_nul(path.as_os_str());
            let mut old_dacl = null_mut();
            let mut descriptor = null_mut();
            let result = unsafe {
                GetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    &mut old_dacl,
                    null_mut(),
                    &mut descriptor,
                )
            };
            if result != 0 {
                return Err(failure(format!(
                    "could not inspect provider path ACL (Windows error {result})"
                )));
            }
            let mut trustee = unsafe { zeroed() };
            unsafe { BuildTrusteeWithSidW(&mut trustee, sid) };
            let entry = EXPLICIT_ACCESS_W {
                grfAccessPermissions: permissions,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: trustee,
            };
            let mut new_dacl = null_mut();
            let result = unsafe { SetEntriesInAclW(1, &entry, old_dacl, &mut new_dacl) };
            if result != 0 {
                unsafe { LocalFree(descriptor) };
                return Err(failure(format!(
                    "could not construct provider path ACL (Windows error {result})"
                )));
            }
            let result = unsafe {
                SetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    new_dacl,
                    null_mut(),
                )
            };
            unsafe { LocalFree(new_dacl.cast()) };
            if result != 0 {
                unsafe { LocalFree(descriptor) };
                return Err(failure(format!(
                    "could not grant provider path access (Windows error {result})"
                )));
            }
            Ok(Self {
                path,
                old_dacl,
                security_descriptor: descriptor,
                restored: false,
            })
        }

        fn restore(&mut self) -> Result<(), ProviderFailure> {
            if self.restored {
                return Ok(());
            }
            let result = unsafe {
                SetNamedSecurityInfoW(
                    self.path.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    self.old_dacl,
                    null_mut(),
                )
            };
            if result != 0 {
                return Err(failure(format!(
                    "could not restore provider path ACL (Windows error {result})"
                )));
            }
            self.restored = true;
            unsafe { LocalFree(self.security_descriptor) };
            self.security_descriptor = null_mut();
            Ok(())
        }
    }
    impl Drop for AclGrant {
        fn drop(&mut self) {
            if !self.restored {
                unsafe {
                    SetNamedSecurityInfoW(
                        self.path.as_mut_ptr(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION,
                        null_mut(),
                        null_mut(),
                        self.old_dacl,
                        null_mut(),
                    );
                    LocalFree(self.security_descriptor);
                }
            }
        }
    }

    struct AttributeList {
        _storage: Vec<usize>,
        ptr: *mut c_void,
        _capabilities: Box<SECURITY_CAPABILITIES>,
        _inherited_handles: Box<[HANDLE]>,
    }
    impl AttributeList {
        fn security_capabilities(
            sid: PSID,
            inherited_handles: &[HANDLE],
        ) -> Result<Self, ProviderFailure> {
            let mut bytes = 0;
            unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes) };
            if bytes == 0 {
                return Err(last_error("could not size provider process attribute list"));
            }
            let words = bytes.div_ceil(size_of::<usize>());
            let mut storage = vec![0usize; words];
            let ptr = storage.as_mut_ptr().cast();
            if unsafe { InitializeProcThreadAttributeList(ptr, 2, 0, &mut bytes) } == 0 {
                return Err(last_error(
                    "could not initialize provider process attribute list",
                ));
            }
            let capabilities = Box::new(SECURITY_CAPABILITIES {
                AppContainerSid: sid,
                Capabilities: null_mut(),
                CapabilityCount: 0,
                Reserved: 0,
            });
            if unsafe {
                UpdateProcThreadAttribute(
                    ptr,
                    0,
                    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                    (capabilities.as_ref() as *const SECURITY_CAPABILITIES).cast(),
                    size_of::<SECURITY_CAPABILITIES>(),
                    null_mut(),
                    null(),
                )
            } == 0
            {
                unsafe { DeleteProcThreadAttributeList(ptr) };
                return Err(last_error(
                    "could not apply AppContainer security capabilities",
                ));
            }
            let inherited_handles = inherited_handles.to_vec().into_boxed_slice();
            if unsafe {
                UpdateProcThreadAttribute(
                    ptr,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    inherited_handles.as_ptr().cast(),
                    std::mem::size_of_val(inherited_handles.as_ref()),
                    null_mut(),
                    null(),
                )
            } == 0
            {
                unsafe { DeleteProcThreadAttributeList(ptr) };
                return Err(last_error("could not restrict provider inherited handles"));
            }
            Ok(Self {
                _storage: storage,
                ptr,
                _capabilities: capabilities,
                _inherited_handles: inherited_handles,
            })
        }
    }
    impl Drop for AttributeList {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.ptr) };
        }
    }

    struct Job(OwnedHandle);
    impl Job {
        fn kill_on_close() -> Result<Self, ProviderFailure> {
            let handle = unsafe { CreateJobObjectW(null(), null()) };
            if handle.is_null() {
                return Err(last_error("could not create provider containment job"));
            }
            let handle = OwnedHandle(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    handle.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            } == 0
            {
                return Err(last_error("could not configure provider containment job"));
            }
            Ok(Self(handle))
        }
    }

    struct Pipe {
        read: OwnedHandle,
        write: OwnedHandle,
    }

    struct InputPipe {
        read: OwnedHandle,
        _write: OwnedHandle,
    }
    impl InputPipe {
        fn new() -> Result<Self, ProviderFailure> {
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            let mut read: HANDLE = null_mut();
            let mut write: HANDLE = null_mut();
            if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
                return Err(last_error("could not create provider input pipe"));
            }
            let read = OwnedHandle(read);
            let write = OwnedHandle(write);
            if unsafe { SetHandleInformation(write.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(last_error("could not make provider input pipe parent-only"));
            }
            Ok(Self {
                read,
                _write: write,
            })
        }
    }
    impl Pipe {
        fn new() -> Result<Self, ProviderFailure> {
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            let mut read: HANDLE = null_mut();
            let mut write: HANDLE = null_mut();
            if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
                return Err(last_error("could not create provider output pipe"));
            }
            let read = OwnedHandle(read);
            let write = OwnedHandle(write);
            if unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(last_error("could not make provider pipe parent-only"));
            }
            Ok(Self { read, write })
        }
    }

    fn reader(
        handle: OwnedHandle,
        limit: usize,
        overflow: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<Result<Vec<u8>, ProviderFailure>> {
        let raw = handle.0 as usize;
        std::mem::forget(handle);
        std::thread::spawn(move || {
            let mut file = unsafe { File::from_raw_handle(raw as *mut c_void) };
            let mut bytes = Vec::with_capacity(limit.min(8192));
            let mut buffer = [0_u8; 8192];
            loop {
                let read = file.read(&mut buffer).map_err(|error| {
                    failure(format!("could not capture provider output: {error}"))
                })?;
                if read == 0 {
                    return Ok(bytes);
                }
                if bytes.len().saturating_add(read) > limit {
                    let remaining = limit.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&buffer[..remaining]);
                    overflow.store(true, Ordering::Release);
                    return Ok(bytes);
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
        })
    }
}

#[cfg(all(test, windows))]
mod windows_sandbox_tests {
    use super::ProviderSandbox;
    use std::ffi::c_void;
    use std::net::TcpListener;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::null_mut;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::{WRITE_DAC, WRITE_OWNER};

    fn system_executable(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32")
            .join(name)
    }

    fn configure_runtime(command: &mut ProviderSandbox<'_>, workspace: &Path) {
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot");
        command
            .writable_root(workspace)
            .env("SystemRoot", &system_root)
            .env("WINDIR", &system_root)
            .env("TEMP", workspace)
            .env("TMP", workspace);
    }

    fn dacl_bytes(path: &Path) -> Vec<u8> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut dacl = null_mut();
        let mut descriptor: *mut c_void = null_mut();
        let result = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(result, 0, "read DACL failed with Windows error {result}");
        let bytes = if dacl.is_null() {
            Vec::new()
        } else {
            let length = unsafe { (*dacl).AclSize as usize };
            unsafe { std::slice::from_raw_parts(dacl.cast::<u8>(), length).to_vec() }
        };
        unsafe { LocalFree(descriptor) };
        bytes
    }

    #[test]
    fn appcontainer_preflight_can_write_only_its_declared_workspace() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        let executable = system_executable("cmd.exe");
        let workspace_dacl = dacl_bytes(&workspace);
        let outside_dacl = dacl_bytes(&outside);

        let mut allowed = ProviderSandbox::new(&executable, &workspace);
        allowed.args(["/D", "/C", "> inside.txt echo contained"]);
        configure_runtime(&mut allowed, &workspace);
        let output = allowed.output().expect("AppContainer preflight launch");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("inside.txt")).expect("inside output"),
            "contained\r\n"
        );
        assert_eq!(
            dacl_bytes(&workspace),
            workspace_dacl,
            "workspace DACL changed"
        );
        assert_eq!(dacl_bytes(&outside), outside_dacl, "outside DACL changed");

        let marker = outside.join("escaped.txt");
        let escape = format!("> \"{}\" echo escaped", marker.display());
        let mut denied = ProviderSandbox::new(&executable, &workspace);
        denied.args(["/D", "/C", &escape]);
        configure_runtime(&mut denied, &workspace);
        let _ = denied.output().expect("contained escape attempt launches");
        assert!(
            !marker.exists(),
            "AppContainer wrote outside its granted root"
        );
        assert_eq!(
            dacl_bytes(&workspace),
            workspace_dacl,
            "workspace DACL changed"
        );
        assert_eq!(dacl_bytes(&outside), outside_dacl, "outside DACL changed");
    }

    #[test]
    fn writable_acl_excludes_acl_and_owner_control_rights() {
        let root = tempfile::tempdir().expect("root");
        let permissions = super::windows_appcontainer::writable_permissions(root.path());
        assert_eq!(permissions & WRITE_DAC, 0, "sandbox may rewrite DACLs");
        assert_eq!(permissions & WRITE_OWNER, 0, "sandbox may take ownership");
    }

    #[test]
    fn appcontainer_runtime_and_output_are_bounded() {
        let executable = system_executable("cmd.exe");

        let runtime_root = tempfile::tempdir().expect("runtime workspace");
        let mut runtime = ProviderSandbox::new(&executable, runtime_root.path());
        runtime.args(["/D", "/C", "for /L %i in (1,0,2) do @rem"]);
        runtime.test_limits(Duration::from_millis(100), 1024);
        configure_runtime(&mut runtime, runtime_root.path());
        let runtime_error = runtime
            .output()
            .expect_err("runtime must be bounded")
            .to_string();
        assert!(runtime_error.contains("runtime limit"), "{runtime_error}");

        let output_root = tempfile::tempdir().expect("output workspace");
        let mut output = ProviderSandbox::new(&executable, output_root.path());
        output.args([
            "/D",
            "/C",
            "for /L %i in (1,1,1000000) do @echo 0123456789abcdef",
        ]);
        output.test_limits(Duration::from_secs(5), 1024);
        configure_runtime(&mut output, output_root.path());
        let output_error = output
            .output()
            .expect_err("output must be bounded")
            .to_string();
        assert!(output_error.contains("output limit"), "{output_error}");
    }

    #[test]
    fn completed_appcontainer_terminates_started_descendants() {
        let executable = system_executable("cmd.exe");
        let root = tempfile::tempdir().expect("workspace");
        std::fs::write(
            root.path().join("parent.cmd"),
            "@start \"\" /b cmd.exe /d /c child.cmd\r\n:wait\r\n@if not exist descendant-started goto wait\r\n@exit /b 0\r\n",
        )
        .expect("parent script");
        std::fs::write(
            root.path().join("child.cmd"),
            "@echo started>descendant-started\r\n:wait\r\n@if not exist descendant-release goto wait\r\n@echo survived>descendant-marker\r\n",
        )
        .expect("child script");
        let mut command = ProviderSandbox::new(&executable, root.path());
        command.args(["/D", "/C", "parent.cmd"]);
        command.test_limits(Duration::from_secs(2), 1024);
        configure_runtime(&mut command, root.path());

        let output = command.output().expect("contained parent launch");

        assert!(output.status.success(), "{output:?}");
        assert!(
            root.path().join("descendant-started").is_file(),
            "test descendant never started"
        );
        std::fs::write(root.path().join("descendant-release"), b"").expect("release marker");
        std::thread::sleep(Duration::from_millis(250));
        assert!(
            !root.path().join("descendant-marker").exists(),
            "provider descendant survived AppContainer completion"
        );
    }

    #[test]
    fn appcontainer_without_capabilities_cannot_connect_to_loopback() {
        let executable = system_executable("curl.exe");
        if !executable.is_file() {
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("address").port();
        let root = tempfile::tempdir().expect("workspace");
        let mut command = ProviderSandbox::new(&executable, root.path());
        command.args([
            "--silent".to_string(),
            "--max-time".to_string(),
            "1".to_string(),
            format!("http://127.0.0.1:{port}/"),
        ]);
        configure_runtime(&mut command, root.path());

        let output = command
            .output()
            .expect("contained network attempt launches");

        assert!(
            !output.status.success(),
            "network attempt unexpectedly succeeded"
        );
        assert!(
            listener.accept().is_err(),
            "AppContainer opened a loopback connection without a network capability"
        );
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn platform_output(_spec: &ProviderSandbox<'_>) -> Result<Output, ProviderFailure> {
    Err(unavailable(
        std::env::consts::OS,
        "this platform has no supported provider process sandbox",
    ))
}
