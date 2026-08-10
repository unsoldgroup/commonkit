use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions as StdOpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(all(test, windows))]
use std::process::Command;
use std::process::Output;

use commonkit_contracts::{Sha256Digest, StableId};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::artifacts::{ArtifactStore, ContentSensitivity};
use super::provider::{
    DesiredStateProvider, ExactProviderVersion, MaterializedState, ProviderCapability,
    ProviderCapabilityResource, ProviderContext, ProviderFailure, ProviderInputs,
    ProviderWorkspace,
};
use super::provider_sandbox::ProviderSandbox;
use super::provider_snapshot::{
    PrivateSnapshotRoot, SymlinkPolicy, copy_regular_file as snapshot_file,
    create_private_snapshot_root,
};
use super::resources::{
    FilesystemIntent, NormalizedManagedPath, NormalizedResource, ResourceProvenance,
};

const APM_PROVIDER_ID: &str = "apm";
const SUPPORTED_APM_VERSION: &str = "0.25.0";
const CONTRACT_VERSION: &str = "commonkit.apm-provider.v1";

#[derive(Debug, Clone)]
pub struct ApmProviderConfig {
    pub executable: PathBuf,
    /// Explicit controller Git executable. When omitted, it is resolved once from the
    /// controller PATH and then pinned by path and digest.
    pub git_executable: Option<PathBuf>,
    pub version: ExactProviderVersion,
    pub manifest: PathBuf,
    pub lockfile: PathBuf,
    pub policy: PathBuf,
    pub targets: Vec<String>,
    pub managed_root: NormalizedManagedPath,
    /// Promoted source whose exact bytes must be part of provider inputs.
    pub bound_source: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ApmProvider {
    id: StableId,
    config: ApmProviderConfig,
    executable_digest: Sha256Digest,
    git_executable: PathBuf,
    git_digest: Sha256Digest,
}

struct ApmInputSnapshot {
    _root: PrivateSnapshotRoot,
    executable: PathBuf,
    git_executable: PathBuf,
    manifest: PathBuf,
    lockfile: PathBuf,
    policy: PathBuf,
    project_sources: Option<PathBuf>,
    promoted_source: Option<PathBuf>,
}

impl ApmProvider {
    pub fn new(mut config: ApmProviderConfig) -> Result<Self, ProviderFailure> {
        if config.version.as_str() != SUPPORTED_APM_VERSION {
            return Err(ProviderFailure::Inspect(format!(
                "unsupported APM provider version {}; CommonKit currently supports exactly {SUPPORTED_APM_VERSION}",
                config.version
            )));
        }
        config.targets.sort();
        config.targets.dedup();
        if config.targets != ["claude", "codex"] {
            return Err(ProviderFailure::Inspect(
                "APM spike supports exactly the claude and codex targets".into(),
            ));
        }
        let executable = ordinary_file(&config.executable, "APM executable")?;
        let executable_digest = digest_bytes(&fs::read(&executable).map_err(|error| {
            ProviderFailure::Inspect(format!(
                "could not read APM executable {}: {error}",
                executable.display()
            ))
        })?)?;
        config.executable = executable;
        let git_executable = resolve_git_executable(config.git_executable.as_deref())?;
        let git_digest = digest_bytes(&fs::read(&git_executable).map_err(|error| {
            ProviderFailure::Inspect(format!(
                "could not read Git executable {}: {error}",
                git_executable.display()
            ))
        })?)?;
        Ok(Self {
            id: StableId::parse(APM_PROVIDER_ID)
                .map_err(|error| ProviderFailure::Inspect(error.to_string()))?,
            config,
            executable_digest,
            git_executable,
            git_digest,
        })
    }

    fn preflight(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure> {
        let executable = ordinary_file(&self.config.executable, "APM executable")?;
        let current_executable_digest = digest_bytes(&fs::read(&executable).map_err(|error| {
            ProviderFailure::Inspect(format!(
                "could not read APM executable {}: {error}",
                executable.display()
            ))
        })?)?;
        if current_executable_digest != self.executable_digest {
            return Err(ProviderFailure::Inspect(format!(
                "APM executable {} changed after provider initialization; reconstruct the provider and approve a new plan",
                executable.display()
            )));
        }
        let git = ordinary_absolute_file(&self.git_executable, "Git executable")?;
        let current_git_digest = digest_bytes(&fs::read(&git).map_err(|error| {
            ProviderFailure::Inspect(format!(
                "could not read Git executable {}: {error}",
                git.display()
            ))
        })?)?;
        if current_git_digest != self.git_digest {
            return Err(ProviderFailure::Inspect(format!(
                "Git executable {} changed after provider initialization; reconstruct the provider and approve a new plan",
                git.display()
            )));
        }
        let manifest = read_input(&self.config.manifest, "manifest")?;
        let lockfile = read_input(&self.config.lockfile, "lockfile")?;
        let policy = read_input(&self.config.policy, "package policy")?;
        let bound_source = self
            .config
            .bound_source
            .as_ref()
            .map(|path| read_input(path, "bound promoted source"))
            .transpose()?;
        if manifest.is_empty() || lockfile.is_empty() || policy.is_empty() {
            return Err(ProviderFailure::Inspect(
                "APM manifest, lockfile, and package policy must be non-empty committed inputs"
                    .into(),
            ));
        }
        // Ensure the configured executable remains part of preflight even though its bytes are
        // deliberately not a portable provider input.
        let _ = executable;

        let mut input_digests = BTreeMap::from([
            ("manifest".into(), digest_bytes(&manifest)?),
            ("lockfile".into(), digest_bytes(&lockfile)?),
            ("packagePolicy".into(), digest_bytes(&policy)?),
            (
                "projectSources".into(),
                digest_optional_tree(
                    &self
                        .config
                        .manifest
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(".apm"),
                )?,
            ),
            ("targetPolicy".into(), context.policy_digest.clone()),
            ("targetPlatform".into(), context.platform_facts_digest()?),
            ("providerExecutable".into(), current_executable_digest),
            ("gitExecutable".into(), current_git_digest),
        ]);
        if let Some(bytes) = bound_source {
            input_digests.insert("promotedSource".into(), digest_bytes(&bytes)?);
        }
        ProviderInputs::new(
            self.id.clone(),
            self.config.version.clone(),
            CONTRACT_VERSION.into(),
            input_digests,
            vec!["agent-context".into(), "claude".into(), "codex".into()],
        )
        .map_err(ProviderFailure::Contract)
    }

    fn run(
        &self,
        snapshot: &ApmInputSnapshot,
        staging: &Path,
        scratch: &Path,
        args: &[&str],
    ) -> Result<Output, ProviderFailure> {
        let staging = provider_cli_path(staging);
        let scratch = provider_cli_path(scratch);
        let runtime = scratch.join("provider-runtime");
        fs::create_dir_all(&runtime).map_err(materialize_io)?;
        set_private_directory(&runtime)?;
        let mut command = ProviderSandbox::new(&snapshot.executable, &staging);
        command
            .args(args)
            .writable_root(&staging)
            .writable_root(&runtime)
            .readable_path(&snapshot.git_executable);
        #[cfg(windows)]
        if let Some(git_installation) = snapshot
            .git_executable
            .parent()
            .and_then(std::path::Path::parent)
        {
            // Git for Windows dispatches helpers from sibling directories under
            // its installation root. Grant that pinned runtime tree read/execute
            // access without granting any repository or user-profile path.
            command.readable_path(git_installation);
        }
        configure_provider_sandbox_environment(
            &mut command,
            &staging,
            &runtime,
            &snapshot.git_executable,
        );
        let output = command.output().map_err(|error| {
            ProviderFailure::Materialize(format!(
                "could not execute pinned APM {} at {}: {error}",
                self.config.version,
                snapshot.executable.display()
            ))
        })?;
        persist_private_diagnostics(&scratch, args[0], &output)?;
        if !output.status.success() {
            return Err(ProviderFailure::Materialize(format!(
                "APM command `{}` failed with status {}; inspect the provider's private diagnostics, fix the inputs, and retry",
                args.join(" "),
                output.status
            )));
        }
        Ok(output)
    }

    fn prepare_staging(
        &self,
        staging: &Path,
        snapshot: &ApmInputSnapshot,
    ) -> Result<(), ProviderFailure> {
        if let Some(entry) = fs::read_dir(staging).map_err(materialize_io)?.next() {
            let entry = entry.map_err(materialize_io)?;
            return Err(ProviderFailure::Materialize(format!(
                "APM staging directory must be empty; found {}",
                entry.file_name().to_string_lossy()
            )));
        }
        copy_input(&snapshot.manifest, &staging.join("apm.yml"))?;
        copy_input(&snapshot.lockfile, &staging.join("apm.lock.yaml"))?;
        copy_input(&snapshot.policy, &staging.join("apm-policy.yml"))?;
        if let Some(source) = &snapshot.project_sources {
            copy_tree(source, &staging.join(".apm"))?;
        }
        Ok(())
    }

    fn snapshot_inputs(
        &self,
        scratch: &Path,
        approved: &ProviderInputs,
    ) -> Result<ApmInputSnapshot, ProviderFailure> {
        let root = create_private_snapshot_root(scratch)?;
        let executable = root.join(
            self.config
                .executable
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("apm")),
        );
        let executable_bytes =
            snapshot_file(&self.config.executable, &executable, "APM executable")?;
        require_snapshot_digest(
            "APM executable",
            &digest_bytes(&executable_bytes)?,
            &approved.input_digests["providerExecutable"],
        )?;

        let git_executable = snapshot_git_runtime(&self.git_executable, &root)?;
        let git_bytes = fs::read(&git_executable).map_err(materialize_io)?;
        require_snapshot_digest(
            "Git executable",
            &digest_bytes(&git_bytes)?,
            &approved.input_digests["gitExecutable"],
        )?;

        let manifest = root.join("apm.yml");
        let lockfile = root.join("apm.lock.yaml");
        let policy = root.join("apm-policy.yml");
        for (source, destination, label, digest_name) in [
            (&self.config.manifest, &manifest, "APM manifest", "manifest"),
            (&self.config.lockfile, &lockfile, "APM lockfile", "lockfile"),
            (
                &self.config.policy,
                &policy,
                "APM package policy",
                "packagePolicy",
            ),
        ] {
            let bytes = snapshot_file(source, destination, label)?;
            require_snapshot_digest(
                label,
                &digest_bytes(&bytes)?,
                &approved.input_digests[digest_name],
            )?;
        }

        let live_sources = self
            .config
            .manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".apm");
        let project_sources = if live_sources.exists() {
            let destination = root.join("project-sources");
            super::provider_snapshot::copy_tree(
                &live_sources,
                &destination,
                SymlinkPolicy::Reject,
            )?;
            Some(destination)
        } else {
            None
        };
        let absent_project_sources = root.join("absent-project-sources");
        let snapshotted_source_digest = digest_optional_tree(
            project_sources
                .as_deref()
                .unwrap_or(&absent_project_sources),
        )?;
        require_snapshot_digest(
            "APM project sources",
            &snapshotted_source_digest,
            &approved.input_digests["projectSources"],
        )?;

        let promoted_source = if let Some(source) = &self.config.bound_source {
            let destination = root.join("promoted-source");
            let bytes = snapshot_file(source, &destination, "APM promoted source")?;
            require_snapshot_digest(
                "APM promoted source",
                &digest_bytes(&bytes)?,
                &approved.input_digests["promotedSource"],
            )?;
            Some(destination)
        } else {
            None
        };
        Ok(ApmInputSnapshot {
            _root: root,
            executable,
            git_executable,
            manifest,
            lockfile,
            policy,
            project_sources,
            promoted_source,
        })
    }
}

impl ApmInputSnapshot {
    fn verify(&self, approved: &ProviderInputs) -> Result<(), ProviderFailure> {
        for (path, label, digest_name) in [
            (&self.executable, "APM executable", "providerExecutable"),
            (&self.git_executable, "Git executable", "gitExecutable"),
            (&self.manifest, "APM manifest", "manifest"),
            (&self.lockfile, "APM lockfile", "lockfile"),
            (&self.policy, "APM package policy", "packagePolicy"),
        ] {
            require_snapshot_digest(
                label,
                &digest_bytes(&fs::read(path).map_err(materialize_io)?)?,
                &approved.input_digests[digest_name],
            )?;
        }
        let absent = self._root.join("absent-project-sources");
        require_snapshot_digest(
            "APM project sources",
            &digest_optional_tree(self.project_sources.as_deref().unwrap_or(&absent))?,
            &approved.input_digests["projectSources"],
        )?;
        if let Some(path) = &self.promoted_source {
            require_snapshot_digest(
                "APM promoted source",
                &digest_bytes(&fs::read(path).map_err(materialize_io)?)?,
                &approved.input_digests["promotedSource"],
            )?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), ProviderFailure> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(materialize_io)
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), ProviderFailure> {
    Ok(())
}

fn require_snapshot_digest(
    label: &str,
    actual: &Sha256Digest,
    approved: &Sha256Digest,
) -> Result<(), ProviderFailure> {
    if actual == approved {
        Ok(())
    } else {
        Err(ProviderFailure::Materialize(format!(
            "{label} changed while creating its immutable snapshot; retry inspection and approve a new plan"
        )))
    }
}

#[cfg(not(windows))]
fn snapshot_git_runtime(source: &Path, root: &Path) -> Result<PathBuf, ProviderFailure> {
    let destination = root.join("git");
    snapshot_file(source, &destination, "Git executable")?;
    Ok(destination)
}

#[cfg(windows)]
fn snapshot_git_runtime(source: &Path, root: &Path) -> Result<PathBuf, ProviderFailure> {
    let installation = source.parent().and_then(Path::parent).ok_or_else(|| {
        ProviderFailure::Materialize("Git executable has no installation root".into())
    })?;
    let destination = root.join("git-runtime");
    super::provider_snapshot::copy_tree(installation, &destination, SymlinkPolicy::Preserve)?;
    let relative = source
        .strip_prefix(installation)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    Ok(destination.join(relative))
}

fn configure_provider_sandbox_environment(
    command: &mut ProviderSandbox<'_>,
    staging: &Path,
    scratch: &Path,
    git: &Path,
) {
    command
        .env("HOME", staging)
        .env("TMPDIR", scratch)
        .env("APM_TEMP_DIR", scratch)
        .env("APM_CACHE_DIR", scratch.join("apm-cache"))
        .env("APM_NON_INTERACTIVE", "1")
        .env("GIT_PYTHON_GIT_EXECUTABLE", git);
    #[cfg(windows)]
    {
        command
            .env("USERPROFILE", staging)
            .env("APPDATA", staging.join("AppData/Roaming"))
            .env("LOCALAPPDATA", staging.join("AppData/Local"))
            .env("TEMP", scratch)
            .env("TMP", scratch);
        if let Some(system_root) =
            std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR"))
        {
            command
                .env("SystemRoot", &system_root)
                .env("WINDIR", &system_root)
                .env("COMSPEC", Path::new(&system_root).join("System32/cmd.exe"));
        }
        command
            .env("OS", "Windows_NT")
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD");
    }
}

fn provider_cli_path(path: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        // APM 0.25.0 appends POSIX-style path fragments internally. Python's
        // Windows path handling rejects those fragments when the working
        // directory uses Rust's verbatim (`\\?\`) representation, so hand the
        // external CLI the equivalent ordinary DOS/UNC spelling.
        let rendered = path.as_os_str().to_string_lossy();
        if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
            return std::path::PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = rendered.strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

fn persist_private_diagnostics(
    scratch: &Path,
    command: &str,
    output: &Output,
) -> Result<(), ProviderFailure> {
    validate_diagnostic_command(command)?;
    for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let path = scratch.join(format!("apm-{command}.{stream}"));
        let mut options = StdOpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(materialize_io)?;
        file.write_all(bytes).map_err(materialize_io)?;
        file.sync_all().map_err(materialize_io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file
                .metadata()
                .map_err(materialize_io)?
                .permissions()
                .mode()
                & 0o777
                != 0o600
            {
                return Err(ProviderFailure::Materialize(
                    "APM private diagnostic permissions are unsafe".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Returns a bounded, line-redacted diagnostic for an explicitly secret-free
/// troubleshooting context. Raw provider output remains private in scratch.
pub fn redacted_apm_diagnostic_summary(
    scratch: &Path,
    command: &str,
) -> Result<String, ProviderFailure> {
    validate_diagnostic_command(command)?;
    const MAX_BYTES: usize = 4096;
    let mut summary = String::new();
    for stream in ["stdout", "stderr"] {
        let bytes =
            fs::read(scratch.join(format!("apm-{command}.{stream}"))).map_err(materialize_io)?;
        let bounded = &bytes[..bytes.len().min(MAX_BYTES)];
        summary.push_str(stream);
        summary.push_str(":\n");
        for line in String::from_utf8_lossy(bounded).lines() {
            let lowercase = line.to_ascii_lowercase();
            if [
                "authorization",
                "bearer ",
                "token",
                "secret",
                "password",
                "api_key",
                "apikey",
            ]
            .iter()
            .any(|marker| lowercase.contains(marker))
            {
                summary.push_str("[REDACTED SENSITIVE LINE]\n");
            } else {
                summary.push_str(line);
                summary.push('\n');
            }
        }
        if bytes.len() > MAX_BYTES {
            summary.push_str("[TRUNCATED]\n");
        }
    }
    Ok(summary)
}

fn validate_diagnostic_command(command: &str) -> Result<(), ProviderFailure> {
    if !command.is_empty()
        && command
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        Ok(())
    } else {
        Err(ProviderFailure::Materialize(
            "invalid APM diagnostic command label".into(),
        ))
    }
}

#[cfg(all(test, windows))]
fn configure_provider_environment(
    command: &mut Command,
    staging: &Path,
    scratch: &Path,
    git: &Path,
) {
    command
        .env_clear()
        .env("HOME", staging)
        .env("TMPDIR", scratch)
        .env("APM_TEMP_DIR", scratch)
        .env("APM_CACHE_DIR", scratch.join("apm-cache"))
        .env("APM_NON_INTERACTIVE", "1")
        .env("GIT_PYTHON_GIT_EXECUTABLE", git);
    #[cfg(windows)]
    {
        use std::path::Component;

        // Python's Windows home lookup does not use HOME. Keep every writable
        // provider location inside staging while restoring only OS runtime
        // paths required by the official PyInstaller bundle.
        command
            .env("USERPROFILE", staging)
            .env("APPDATA", staging.join("AppData/Roaming"))
            .env("LOCALAPPDATA", staging.join("AppData/Local"))
            .env("TEMP", scratch)
            .env("TMP", scratch);
        if let Some(system_root) =
            std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR"))
        {
            command
                .env("SystemRoot", &system_root)
                .env("WINDIR", &system_root)
                .env("COMSPEC", Path::new(&system_root).join("System32/cmd.exe"));
        }
        command
            .env("OS", "Windows_NT")
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD");
        if let Some(Component::Prefix(prefix)) = staging.components().next() {
            let drive = prefix.as_os_str();
            if let Ok(home_path) = staging.strip_prefix(Path::new(drive)) {
                command
                    .env("HOMEDRIVE", drive)
                    .env("HOMEPATH", home_path.as_os_str());
            }
        }
    }
}

#[cfg(all(test, windows))]
mod windows_environment_tests {
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn provider_cli_paths_remove_verbatim_prefixes() {
        assert_eq!(
            super::provider_cli_path(Path::new(r"\\?\C:\isolated\stage")),
            PathBuf::from(r"C:\isolated\stage")
        );
        assert_eq!(
            super::provider_cli_path(Path::new(r"\\?\UNC\server\share\stage")),
            PathBuf::from(r"\\server\share\stage")
        );
    }

    #[test]
    fn provider_output_paths_use_portable_separators() {
        assert_eq!(
            super::portable_relative_path(Path::new(r"rules\base.md")).unwrap(),
            "rules/base.md"
        );
    }

    use super::*;

    #[test]
    fn pinned_bundle_receives_only_isolated_windows_profile_and_temp_paths() {
        let staging = Path::new(r"C:\isolated\stage");
        let scratch = Path::new(r"C:\isolated\scratch");
        let git = Path::new(r"C:\Program Files\Git\cmd\git.exe");
        let mut command = Command::new("apm.exe");
        configure_provider_environment(&mut command, staging, scratch, git);
        let explicit = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect::<BTreeMap<OsString, OsString>>();

        for (key, expected) in [
            ("HOME", staging.as_os_str()),
            ("USERPROFILE", staging.as_os_str()),
            ("TEMP", scratch.as_os_str()),
            ("TMP", scratch.as_os_str()),
            ("TMPDIR", scratch.as_os_str()),
        ] {
            assert_eq!(
                explicit.get(OsStr::new(key)),
                Some(&expected.to_owned()),
                "{key}"
            );
        }
        assert_eq!(
            explicit.get(OsStr::new("APPDATA")),
            Some(&staging.join("AppData/Roaming").into_os_string())
        );
        assert_eq!(
            explicit.get(OsStr::new("LOCALAPPDATA")),
            Some(&staging.join("AppData/Local").into_os_string())
        );
        let system_root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR"));
        assert_eq!(explicit.get(OsStr::new("SystemRoot")), system_root.as_ref());
        assert_eq!(explicit.get(OsStr::new("WINDIR")), system_root.as_ref());
        assert!(explicit.contains_key(OsStr::new("COMSPEC")));
        assert_eq!(
            explicit.get(OsStr::new("PATHEXT")),
            Some(&OsString::from(".COM;.EXE;.BAT;.CMD"))
        );
        assert!(!explicit.contains_key(OsStr::new("PATH")));
        assert!(!explicit.contains_key(OsStr::new("GITHUB_TOKEN")));
        assert_eq!(
            explicit.get(OsStr::new("GIT_PYTHON_GIT_EXECUTABLE")),
            Some(&git.as_os_str().to_owned())
        );
    }
}

impl DesiredStateProvider for ApmProvider {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn inspect_inputs(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure> {
        self.preflight(context)
    }

    fn materialize(
        &self,
        context: &ProviderContext,
        workspace: &ProviderWorkspace,
        artifacts: &ArtifactStore,
    ) -> Result<MaterializedState, ProviderFailure> {
        let inputs = self.preflight(context)?;
        let staging = workspace.staging_root();
        let scratch = workspace.scratch_root();
        let snapshot = self.snapshot_inputs(scratch, &inputs)?;
        snapshot.verify(&inputs)?;
        self.prepare_staging(staging, &snapshot)?;

        let version = self.run(&snapshot, staging, scratch, &["--version"])?;
        snapshot.verify(&inputs)?;
        let found = parse_version(&String::from_utf8_lossy(&version.stdout));
        if found.as_deref() != Some(self.config.version.as_str()) {
            return Err(ProviderFailure::Materialize(format!(
                "expected APM {}, found {}; install the exact configured release or update the loadout",
                self.config.version,
                found.unwrap_or_else(|| "an unrecognized version".into())
            )));
        }

        let targets = self.config.targets.join(",");
        self.run(
            &snapshot,
            staging,
            scratch,
            &["install", "--frozen", "--target", &targets],
        )?;
        snapshot.verify(&inputs)?;
        self.run(
            &snapshot,
            staging,
            scratch,
            &["compile", "--target", &targets],
        )?;
        snapshot.verify(&inputs)?;
        let policy = staging.join("apm-policy.yml");
        let policy = policy.to_str().ok_or_else(|| {
            ProviderFailure::Materialize(
                "APM staging policy path cannot be represented as UTF-8".into(),
            )
        })?;
        let audit = self.run(
            &snapshot,
            staging,
            scratch,
            &[
                "audit",
                "--ci",
                "--policy",
                policy,
                "--no-fail-fast",
                "--format",
                "json",
            ],
        )?;
        snapshot.verify(&inputs)?;
        serde_json::from_slice::<serde_json::Value>(&audit.stdout).map_err(|error| {
            ProviderFailure::Materialize(format!(
                "APM audit did not return valid JSON for the staged output: {error}"
            ))
        })?;

        let capabilities = scan_mcp_outputs(staging, &inputs)?;
        let resources = scan_outputs(staging, &self.config.managed_root, &inputs, artifacts)?;
        if resources.is_empty() {
            return Err(ProviderFailure::Materialize(
                "APM produced no Claude or Codex files in isolated staging".into(),
            ));
        }
        MaterializedState::finalize_with_capabilities(
            inputs,
            resources,
            Vec::new(),
            Vec::new(),
            capabilities,
        )
        .map_err(ProviderFailure::Contract)
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ApmManifest {
    #[serde(default)]
    dependencies: ApmDependencies,
    #[serde(default)]
    dev_dependencies: ApmDependencies,
}

#[derive(Debug, Deserialize, Default)]
struct ApmDependencies {
    #[serde(default)]
    mcp: Vec<ApmMcpEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ApmMcpEntry {
    Registry(String),
    Object(ApmMcpObject),
}

#[derive(Debug, Deserialize)]
struct ApmMcpObject {
    name: String,
    #[serde(default)]
    registry: Option<serde_yaml::Value>,
    transport: Option<String>,
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StagedMcpConfig {
    mcp_servers: BTreeMap<String, StagedMcpServer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagedMcpServer {
    #[serde(rename = "type")]
    transport: String,
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

fn scan_mcp_outputs(
    staging: &Path,
    inputs: &ProviderInputs,
) -> Result<Vec<ProviderCapabilityResource>, ProviderFailure> {
    let manifest: ApmManifest =
        serde_yaml::from_slice(&fs::read(staging.join("apm.yml")).map_err(materialize_io)?)
            .map_err(|error| {
                ProviderFailure::Materialize(format!(
                    "invalid documented APM MCP manifest shape: {error}"
                ))
            })?;
    let mut declared = BTreeMap::new();
    for entry in manifest
        .dependencies
        .mcp
        .into_iter()
        .chain(manifest.dev_dependencies.mcp)
    {
        let (name, direct) = match entry {
            ApmMcpEntry::Registry(name) => (name, None),
            ApmMcpEntry::Object(object) => {
                if !object.extra.is_empty() {
                    return Err(ProviderFailure::Materialize(format!(
                        "APM MCP declaration {} contains passthrough fields that CommonKit cannot safely translate",
                        object.name
                    )));
                }
                let is_self_defined =
                    matches!(object.registry, Some(serde_yaml::Value::Bool(false)));
                let direct = if is_self_defined {
                    let transport = object.transport.as_deref().ok_or_else(|| {
                        ProviderFailure::Materialize(format!(
                            "APM MCP declaration {} has ambiguous transport",
                            object.name
                        ))
                    })?;
                    if !matches!(transport, "http" | "streamable-http") {
                        return Err(ProviderFailure::Materialize(format!(
                            "APM MCP declaration {} uses unsupported relay transport {transport}",
                            object.name
                        )));
                    }
                    Some((
                        object.url.ok_or_else(|| {
                            ProviderFailure::Materialize(format!(
                                "APM MCP declaration {} has no resolved URL",
                                object.name
                            ))
                        })?,
                        object.headers,
                    ))
                } else {
                    None
                };
                (object.name, direct)
            }
        };
        if name.trim().is_empty() || declared.insert(name.clone(), direct).is_some() {
            return Err(ProviderFailure::Materialize(format!(
                "ambiguous duplicate APM MCP declaration {name}"
            )));
        }
    }
    let staged_path = staging.join(".mcp.json");
    if declared.is_empty() {
        if staged_path.exists() {
            return Err(ProviderFailure::Materialize(
                "APM staged MCP output without a manifest declaration".into(),
            ));
        }
        return Ok(Vec::new());
    }
    let staged: StagedMcpConfig =
        serde_json::from_slice(&fs::read(&staged_path).map_err(|_| {
            ProviderFailure::Materialize(
                "APM declared MCP servers but did not stage documented .mcp.json output".into(),
            )
        })?)
        .map_err(|error| {
            ProviderFailure::Materialize(format!("invalid staged APM MCP output: {error}"))
        })?;
    if declared.len() != staged.mcp_servers.len() {
        return Err(ProviderFailure::Materialize(
            "APM MCP manifest and staged output are ambiguous".into(),
        ));
    }
    let mut capabilities = Vec::new();
    for (name, direct) in declared {
        let resolved = staged.mcp_servers.get(&name).ok_or_else(|| {
            ProviderFailure::Materialize(format!(
                "APM MCP declaration {name} was not present in staged output"
            ))
        })?;
        if !matches!(resolved.transport.as_str(), "http" | "streamable-http") {
            return Err(ProviderFailure::Materialize(format!(
                "APM MCP declaration {name} resolved to unsupported relay transport {}",
                resolved.transport
            )));
        }
        if let Some((url, headers)) = direct {
            if url != resolved.url || headers != resolved.headers {
                return Err(ProviderFailure::Materialize(format!(
                    "APM MCP declaration {name} conflicts with staged output"
                )));
            }
        }
        let headers = resolved
            .headers
            .iter()
            .map(|(key, value)| Ok((key.clone(), normalize_apm_secret_reference(value)?)))
            .collect::<Result<BTreeMap<_, _>, ProviderFailure>>()?;
        capabilities.push(ProviderCapabilityResource {
            capability: ProviderCapability::McpStreamableHttp {
                id: name.clone(),
                name: name.clone(),
                enabled: true,
                url: resolved.url.clone(),
                headers,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: format!("apm.yml:dependencies.mcp[{name}] -> .mcp.json:mcpServers.{name}"),
            },
        });
    }
    Ok(capabilities)
}

fn normalize_apm_secret_reference(value: &str) -> Result<String, ProviderFailure> {
    let variable = value
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| {
            value
                .strip_prefix("${")
                .and_then(|value| value.strip_suffix('}'))
        });
    let Some(variable) = variable else {
        return Err(ProviderFailure::Materialize(
            "APM MCP headers must use a portable environment reference; literal values and input prompts are unsupported".into(),
        ));
    };
    if variable.is_empty()
        || variable.starts_with("input:")
        || !variable
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(ProviderFailure::Materialize(
            "APM MCP header contains an unsupported credential reference".into(),
        ));
    }
    Ok(format!("env:{variable}"))
}

fn scan_outputs(
    staging: &Path,
    managed_root: &NormalizedManagedPath,
    inputs: &ProviderInputs,
    artifacts: &ArtifactStore,
) -> Result<Vec<NormalizedResource>, ProviderFailure> {
    let mut resources = Vec::new();
    for output_root in [".claude", ".codex", ".agents"] {
        let root = staging.join(output_root);
        if !root.exists() {
            continue;
        }
        let mut staged = Vec::new();
        collect_files(&root, &root, &mut staged)?;
        for (relative, bytes) in staged {
            let source = format!("{output_root}/{}", portable_relative_path(&relative)?);
            let path = NormalizedManagedPath::parse(format!("{}/{source}", managed_root.as_str()))
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            let content = artifacts
                .put(&bytes, ContentSensitivity::Portable)
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            resources.push(NormalizedResource {
                intent: (FilesystemIntent::File {
                    path,
                    content,
                    mode: None,
                    expected_before: None,
                })
                .into(),
                provenance: ResourceProvenance {
                    provider_id: inputs.provider_id.clone(),
                    provider_version: inputs.provider_version.to_string(),
                    input_digest: inputs.input_set_digest.clone(),
                    source,
                },
            });
        }
    }
    for output_file in ["AGENTS.md", "CLAUDE.md"] {
        let file = staging.join(output_file);
        if !file.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&file).map_err(materialize_io)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ProviderFailure::Materialize(format!(
                "APM staged unsupported root output {output_file}"
            )));
        }
        let content = artifacts
            .put(
                &fs::read(&file).map_err(materialize_io)?,
                ContentSensitivity::Portable,
            )
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
        resources.push(NormalizedResource {
            intent: (FilesystemIntent::File {
                path: NormalizedManagedPath::parse(format!(
                    "{}/{output_file}",
                    managed_root.as_str()
                ))
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?,
                content,
                mode: None,
                expected_before: None,
            })
            .into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: output_file.into(),
            },
        });
    }
    resources.sort_by_key(NormalizedResource::sort_key);
    Ok(resources)
}

fn portable_relative_path(path: &Path) -> Result<String, ProviderFailure> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| {
                        ProviderFailure::Materialize(
                            "APM output path cannot be represented as portable UTF-8".into(),
                        )
                    })?
                    .to_owned(),
            ),
            _ => {
                return Err(ProviderFailure::Materialize(
                    "APM output path is not a portable relative path".into(),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(ProviderFailure::Materialize(
            "APM output path cannot be empty".into(),
        ));
    }
    Ok(parts.join("/"))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), ProviderFailure> {
    fs::create_dir_all(destination).map_err(materialize_io)?;
    let mut entries = fs::read_dir(source)
        .map_err(materialize_io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(materialize_io)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path()).map_err(materialize_io)?;
        let target = destination.join(entry.file_name());
        if metadata.file_type().is_symlink() {
            return Err(ProviderFailure::Inspect(format!(
                "APM project source contains unsupported symlink {}",
                entry.path().display()
            )));
        } else if metadata.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), target).map_err(materialize_io)?;
        } else {
            return Err(ProviderFailure::Inspect(format!(
                "APM project source contains unsupported filesystem object {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn digest_optional_tree(root: &Path) -> Result<Sha256Digest, ProviderFailure> {
    let mut hasher = Sha256::new();
    hasher.update(b"commonkit.apm-project-sources.v1\0");
    if root.exists() {
        let mut files = Vec::new();
        collect_files(root, root, &mut files)?;
        for (path, bytes) in files {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update([0]);
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
    }
    Sha256Digest::parse(format!("sha256:{:x}", hasher.finalize()))
        .map_err(|error| ProviderFailure::Inspect(error.to_string()))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<(), ProviderFailure> {
    let mut entries = fs::read_dir(directory)
        .map_err(materialize_io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(materialize_io)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path()).map_err(materialize_io)?;
        if metadata.file_type().is_symlink() {
            return Err(ProviderFailure::Materialize(format!(
                "APM staged a symlink at {}; symlink output is unsupported by the 0.25.0 spike",
                entry.path().display()
            )));
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if metadata.is_file() {
            output.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| ProviderFailure::Materialize(error.to_string()))?
                    .to_path_buf(),
                fs::read(entry.path()).map_err(materialize_io)?,
            ));
        } else {
            return Err(ProviderFailure::Materialize(format!(
                "APM staged unsupported filesystem object {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn parse_version(stdout: &str) -> Option<String> {
    stdout
        .trim()
        .strip_prefix("Agent Package Manager (APM) CLI version ")?
        .split_whitespace()
        .next()
        .filter(|version| {
            let parts = version.split('.').collect::<Vec<_>>();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        })
        .map(str::to_owned)
}

fn ordinary_file(path: &Path, label: &str) -> Result<PathBuf, ProviderFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ProviderFailure::Inspect(format!(
            "{label} {} is unavailable: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ProviderFailure::Inspect(format!(
            "{label} {} must be an ordinary file",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}

fn ordinary_absolute_file(path: &Path, label: &str) -> Result<PathBuf, ProviderFailure> {
    if !path.is_absolute() {
        return Err(ProviderFailure::Inspect(format!(
            "{label} {} must be an absolute path",
            path.display()
        )));
    }
    ordinary_file(path, label)
}

fn resolve_git_executable(explicit: Option<&Path>) -> Result<PathBuf, ProviderFailure> {
    if let Some(path) = explicit {
        return ordinary_absolute_file(path, "Git executable");
    }
    let path = std::env::var_os("PATH").ok_or_else(|| {
        ProviderFailure::Inspect(
            "APM requires Git; configure an absolute ordinary Git executable or add Git to the controller PATH".into(),
        )
    })?;
    let filename = if cfg!(windows) { "git.exe" } else { "git" };
    for directory in std::env::split_paths(&path).filter(|path| path.is_absolute()) {
        let candidate = directory.join(filename);
        if ordinary_absolute_file(&candidate, "Git executable").is_ok() {
            return Ok(candidate);
        }
    }
    Err(ProviderFailure::Inspect(
        "APM requires Git; configure an absolute ordinary Git executable or install Git on the controller PATH".into(),
    ))
}

fn read_input(path: &Path, label: &str) -> Result<Vec<u8>, ProviderFailure> {
    ordinary_file(path, &format!("APM {label}"))?;
    fs::read(path).map_err(|error| {
        ProviderFailure::Inspect(format!(
            "could not read APM {label} {}: {error}",
            path.display()
        ))
    })
}

fn copy_input(source: &Path, destination: &Path) -> Result<(), ProviderFailure> {
    let bytes = fs::read(source).map_err(materialize_io)?;
    fs::write(destination, bytes).map_err(materialize_io)
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, ProviderFailure> {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(|error| ProviderFailure::Inspect(error.to_string()))
}

fn materialize_io(error: std::io::Error) -> ProviderFailure {
    ProviderFailure::Materialize(error.to_string())
}

#[cfg(all(test, unix))]
mod git_dependency_tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn explicit_git_must_exist_and_must_not_be_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing-git");
        assert!(resolve_git_executable(Some(&missing)).is_err());

        let target = root.path().join("git-target");
        fs::write(&target, b"git").unwrap();
        let link = root.path().join("git-link");
        symlink(&target, &link).unwrap();
        assert!(resolve_git_executable(Some(&link)).is_err());
    }

    #[test]
    fn explicit_git_must_be_absolute() {
        assert!(resolve_git_executable(Some(Path::new("git"))).is_err());
    }
}
