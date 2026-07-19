use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use commonkit_contracts::{Sha256Digest, StableId};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::artifacts::{ArtifactStore, ContentSensitivity};
use super::provider::{
    DesiredStateProvider, ExactProviderVersion, MaterializedState, ProviderCapability,
    ProviderCapabilityResource, ProviderContext, ProviderFailure, ProviderInputs,
    ProviderWorkspace,
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
    git_executable: PathBuf,
    git_digest: Sha256Digest,
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
            git_executable,
            git_digest,
        })
    }

    fn preflight(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure> {
        let executable = ordinary_file(&self.config.executable, "APM executable")?;
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
        staging: &Path,
        scratch: &Path,
        args: &[&str],
    ) -> Result<Output, ProviderFailure> {
        let mut command = Command::new(&self.config.executable);
        command.args(args).current_dir(staging);
        configure_provider_environment(&mut command, staging, scratch, &self.git_executable);
        let output = command.output().map_err(|error| {
            ProviderFailure::Materialize(format!(
                "could not execute pinned APM {} at {}: {error}",
                self.config.version,
                self.config.executable.display()
            ))
        })?;
        if !output.status.success() {
            return Err(ProviderFailure::Materialize(format!(
                "APM command `{}` failed with status {}; inspect the provider's private diagnostics, fix the inputs, and retry",
                args.join(" "),
                output.status
            )));
        }
        Ok(output)
    }

    fn prepare_staging(&self, staging: &Path) -> Result<(), ProviderFailure> {
        if let Some(entry) = fs::read_dir(staging).map_err(materialize_io)?.next() {
            let entry = entry.map_err(materialize_io)?;
            return Err(ProviderFailure::Materialize(format!(
                "APM staging directory must be empty; found {}",
                entry.file_name().to_string_lossy()
            )));
        }
        copy_input(&self.config.manifest, &staging.join("apm.yml"))?;
        copy_input(&self.config.lockfile, &staging.join("apm.lock.yaml"))?;
        copy_input(&self.config.policy, &staging.join("apm-policy.yml"))?;
        let source = self
            .config
            .manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".apm");
        if source.exists() {
            copy_tree(&source, &staging.join(".apm"))?;
        }
        Ok(())
    }
}

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
        self.prepare_staging(staging)?;

        let version = self.run(staging, scratch, &["--version"])?;
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
            staging,
            scratch,
            &["install", "--frozen", "--target", &targets],
        )?;
        self.run(staging, scratch, &["compile", "--target", &targets])?;
        let policy = staging.join("apm-policy.yml");
        let policy = policy.to_str().ok_or_else(|| {
            ProviderFailure::Materialize(
                "APM staging policy path cannot be represented as UTF-8".into(),
            )
        })?;
        let audit = self.run(
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
            let source = format!("{output_root}/{}", relative.to_string_lossy());
            let path = NormalizedManagedPath::parse(format!("{}/{source}", managed_root.as_str()))
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            let content = artifacts
                .put(&bytes, ContentSensitivity::Portable)
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            resources.push(NormalizedResource {
                intent: FilesystemIntent::File {
                    path,
                    content,
                    mode: None,
                    expected_before: None,
                },
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
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse(format!(
                    "{}/{output_file}",
                    managed_root.as_str()
                ))
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?,
                content,
                mode: None,
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: output_file.into(),
            },
        });
    }
    resources.sort_by(|left, right| left.intent.path().cmp(right.intent.path()));
    Ok(resources)
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
