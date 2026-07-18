use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use commonkit_contracts::{Sha256Digest, StableId};
use sha2::{Digest, Sha256};

use super::artifacts::{ArtifactStore, ContentSensitivity};
use super::provider::{
    DesiredStateProvider, ExactProviderVersion, MaterializedState, ProviderContext,
    ProviderFailure, ProviderInputs, ProviderWorkspace,
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
    pub version: ExactProviderVersion,
    pub manifest: PathBuf,
    pub lockfile: PathBuf,
    pub policy: PathBuf,
    pub targets: Vec<String>,
    pub managed_root: NormalizedManagedPath,
}

#[derive(Debug, Clone)]
pub struct ApmProvider {
    id: StableId,
    config: ApmProviderConfig,
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
        Ok(Self {
            id: StableId::parse(APM_PROVIDER_ID)
                .map_err(|error| ProviderFailure::Inspect(error.to_string()))?,
            config,
        })
    }

    fn preflight(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure> {
        let executable = ordinary_file(&self.config.executable, "APM executable")?;
        let manifest = read_input(&self.config.manifest, "manifest")?;
        let lockfile = read_input(&self.config.lockfile, "lockfile")?;
        let policy = read_input(&self.config.policy, "package policy")?;
        if manifest.is_empty() || lockfile.is_empty() || policy.is_empty() {
            return Err(ProviderFailure::Inspect(
                "APM manifest, lockfile, and package policy must be non-empty committed inputs"
                    .into(),
            ));
        }
        // Ensure the configured executable remains part of preflight even though its bytes are
        // deliberately not a portable provider input.
        let _ = executable;

        ProviderInputs::new(
            self.id.clone(),
            self.config.version.clone(),
            CONTRACT_VERSION.into(),
            BTreeMap::from([
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
            ]),
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
        let output = Command::new(&self.config.executable)
            .args(args)
            .current_dir(staging)
            .env_clear()
            .env("HOME", staging)
            .env("TMPDIR", scratch)
            .output()
            .map_err(|error| {
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
        for entry in fs::read_dir(staging).map_err(materialize_io)? {
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

        let resources = scan_outputs(staging, &self.config.managed_root, &inputs, artifacts)?;
        if resources.is_empty() {
            return Err(ProviderFailure::Materialize(
                "APM produced no Claude or Codex files in isolated staging".into(),
            ));
        }
        MaterializedState::finalize(inputs, resources, Vec::new(), Vec::new())
            .map_err(ProviderFailure::Contract)
    }
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
