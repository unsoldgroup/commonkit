use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::provider::ProviderFailure;

pub(crate) struct ProviderSandbox<'a> {
    executable: &'a Path,
    args: Vec<OsString>,
    environment: BTreeMap<OsString, OsString>,
    current_dir: &'a Path,
    writable_roots: Vec<PathBuf>,
    readable_paths: Vec<PathBuf>,
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

    pub(crate) fn output(&self) -> Result<Output, ProviderFailure> {
        platform_output(self)
    }
}

fn unavailable(platform: &str, remediation: &str) -> ProviderFailure {
    ProviderFailure::Materialize(format!(
        "provider sandbox capability is unavailable on {platform}; {remediation}; provider execution was denied before launch"
    ))
}

#[cfg(target_os = "macos")]
fn platform_output(spec: &ProviderSandbox<'_>) -> Result<Output, ProviderFailure> {
    let launcher = Path::new("/usr/bin/sandbox-exec");
    if !launcher.is_file() {
        return Err(unavailable(
            "macOS",
            "install a CommonKit build with the required Seatbelt launcher",
        ));
    }
    let mut profile = String::from(
        "(version 1)\n(allow default)\n(deny network*)\n(deny mach-lookup)\n(deny signal)\n(deny ipc-posix*)\n(deny file-write*)\n(deny process-exec)\n",
    );
    // Keep system runtime reads, but remove ambient authority over user data,
    // temporary peers, mounted volumes, and conventional secret locations.
    // The exact declared provider paths are granted back below.
    for sensitive in [
        "/Users",
        "/private/var/folders",
        "/tmp",
        "/var/tmp",
        "/Volumes",
        "/root",
        "/run/secrets",
    ] {
        profile.push_str(&format!(
            "(deny file-read* (subpath {}))\n",
            scheme(sensitive)
        ));
    }
    for runtime in ["/bin", "/usr/bin"] {
        profile.push_str(&format!(
            "(allow process-exec (subpath {}))\n",
            scheme(runtime)
        ));
    }
    let provider_executable = canonical_existing(spec.executable)?;
    let current_dir = canonical_existing(spec.current_dir)?;
    for executable in [provider_executable.as_path(), Path::new("/bin/sh")] {
        profile.push_str(&format!(
            "(allow process-exec (literal {}))\n(allow file-read* (literal {}))\n",
            scheme_path(executable),
            scheme_path(executable)
        ));
    }
    for path in &spec.readable_paths {
        let path = canonical_existing(path)?;
        let selector = if path.is_dir() { "subpath" } else { "literal" };
        profile.push_str(&format!(
            "(allow file-read* ({selector} {}))\n",
            scheme_path(&path)
        ));
        if path.is_file() {
            profile.push_str(&format!(
                "(allow process-exec (literal {}))\n",
                scheme_path(&path)
            ));
        }
    }
    for path in &spec.writable_roots {
        let path = canonical_existing(path)?;
        profile.push_str(&format!(
            "(allow file-read* file-write* (subpath {}))\n",
            scheme_path(&path)
        ));
    }

    let mut command = Command::new(launcher);
    command
        .arg("-p")
        .arg(profile)
        .arg(&provider_executable)
        .args(&spec.args)
        .current_dir(current_dir)
        .env_clear()
        .envs(&spec.environment);
    command.output().map_err(|error| {
        ProviderFailure::Materialize(format!("could not launch macOS provider sandbox: {error}"))
    })
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
    command.output().map_err(|error| {
        ProviderFailure::Materialize(format!("could not launch Linux provider sandbox: {error}"))
    })
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
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, LocalFree,
        SetHandleInformation, WAIT_FAILED,
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
        FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
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
                add_permission(&mut permissions, entry, FILE_ALL_ACCESS);
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
        let job = Job::kill_on_close()?;

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
        if unsafe { AssignProcessToJobObject(job.0.0, process_handle.0) } == 0 {
            unsafe { TerminateProcess(process_handle.0, 1) };
            return Err(last_error(
                "could not attach provider process to containment job",
            ));
        }

        // The parent must release its writer copies before readers can observe EOF.
        drop(stdout_pipe.write);
        drop(stderr_pipe.write);
        let stdout_reader = reader(stdout_pipe.read);
        let stderr_reader = reader(stderr_pipe.read);
        if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
            unsafe { TerminateProcess(process_handle.0, 1) };
            return Err(last_error("could not resume contained provider process"));
        }
        drop(thread_handle);
        if unsafe { WaitForSingleObject(process_handle.0, INFINITE) } == WAIT_FAILED {
            return Err(last_error("could not wait for contained provider process"));
        }
        let mut exit_code = 0;
        if unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) } == 0 {
            return Err(last_error("could not read contained provider exit status"));
        }
        // Close the job before joining pipe readers. A malicious provider may leave a
        // descendant holding inherited writer handles; kill-on-close guarantees EOF.
        drop(job);
        let stdout = stdout_reader
            .join()
            .map_err(|_| failure("provider stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| failure("provider stderr reader panicked"))??;

        // Explicit drops make the security cleanup order auditable: terminate descendants,
        // release process handles, restore every DACL, then release the derived SID.
        drop(process_handle);
        for grant in grants.iter_mut().rev() {
            grant.restore()?;
        }
        drop(grants);
        drop(sid);
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
                let metadata = child.symlink_metadata().map_err(|error| {
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
            if self.0 != 0 && self.0 != INVALID_HANDLE_VALUE {
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
            if handle == 0 {
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
            let mut read = 0;
            let mut write = 0;
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
            let mut read = 0;
            let mut write = 0;
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

    fn reader(handle: OwnedHandle) -> std::thread::JoinHandle<Result<Vec<u8>, ProviderFailure>> {
        let raw = handle.0 as usize;
        std::mem::forget(handle);
        std::thread::spawn(move || {
            let mut file = unsafe { File::from_raw_handle(raw as *mut c_void) };
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|error| failure(format!("could not capture provider output: {error}")))?;
            Ok(bytes)
        })
    }
}

#[cfg(all(test, windows))]
mod windows_sandbox_tests {
    use super::ProviderSandbox;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};

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

    #[test]
    fn appcontainer_preflight_can_write_only_its_declared_workspace() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        let executable = system_executable("cmd.exe");

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

#[cfg(target_os = "macos")]
fn canonical_existing(path: &Path) -> Result<PathBuf, ProviderFailure> {
    path.canonicalize().map_err(|error| {
        ProviderFailure::Materialize(format!(
            "provider sandbox could not canonicalize {}: {error}",
            path.display()
        ))
    })
}

#[cfg(target_os = "macos")]
fn scheme_path(path: &Path) -> String {
    scheme(&path.to_string_lossy())
}

#[cfg(target_os = "macos")]
fn scheme(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string escaping is valid Scheme string escaping")
}
