use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};

use super::artifacts::{ArtifactStore, ContentSensitivity};
use super::provider::{
    DesiredStateProvider, ExactProviderVersion, MaterializedState, ProviderContext,
    ProviderFailure, ProviderInputs, ProviderWorkspace,
};
use super::resources::{
    FileMode, FilesystemIntent, NormalizedManagedPath, NormalizedResource, ResourceProvenance,
    SafeSymlinkTarget,
};

const CHEZMOI_VERSION: &str = "2.70.4";

#[derive(Debug, Clone)]
pub struct ChezmoiProvider {
    id: StableId,
    executable: PathBuf,
    source: PathBuf,
    config: PathBuf,
    cache: PathBuf,
    persistent_state: PathBuf,
    working_tree: PathBuf,
}

impl ChezmoiProvider {
    pub fn new(
        executable: impl AsRef<Path>,
        source: impl AsRef<Path>,
        config: impl AsRef<Path>,
        cache: impl AsRef<Path>,
        persistent_state: impl AsRef<Path>,
        working_tree: impl AsRef<Path>,
    ) -> Result<Self, ProviderFailure> {
        let executable = executable.as_ref().to_path_buf();
        let source = source.as_ref().to_path_buf();
        let config = config.as_ref().to_path_buf();
        if !executable.is_file() || !source.is_dir() || !config.is_file() {
            return Err(ProviderFailure::Inspect(
                "chezmoi requires an existing executable, source directory, and config file".into(),
            ));
        }
        Ok(Self {
            id: StableId::parse("chezmoi")
                .map_err(|error| ProviderFailure::Inspect(error.to_string()))?,
            executable,
            source,
            config,
            cache: cache.as_ref().to_path_buf(),
            persistent_state: persistent_state.as_ref().to_path_buf(),
            working_tree: working_tree.as_ref().to_path_buf(),
        })
    }

    fn preflight(&self) -> Result<(), ProviderFailure> {
        let config = fs::read_to_string(&self.config).map_err(|error| {
            ProviderFailure::Inspect(format!("chezmoi config {}: {error}", self.config.display()))
        })?;
        if ["[hooks.", "command =", "interpreter ="]
            .iter()
            .any(|marker| config.contains(marker))
        {
            return Err(unsupported("config", "hook_or_command"));
        }
        let mut entries = collect_paths(&self.source)
            .map_err(|error| ProviderFailure::Inspect(error.to_string()))?;
        entries.sort();
        for path in entries {
            let relative = path.strip_prefix(&self.source).unwrap_or(&path);
            let source = portable_path(relative);
            let name = relative
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let capability = if name.starts_with("run_") || name == ".chezmoiscripts" {
                Some("script")
            } else if name.starts_with("modify_") {
                Some("modify")
            } else if name.starts_with("create_") {
                Some("create")
            } else if name.starts_with("exact_") {
                Some("exact_directory")
            } else if name.starts_with("remove_") {
                Some("removal")
            } else if name.starts_with(".chezmoiexternal") {
                Some("external")
            } else if name.ends_with(".age") || name.ends_with(".gpg") {
                Some("encrypted_source")
            } else {
                None
            };
            if let Some(capability) = capability {
                return Err(unsupported(&source, capability));
            }
            if path.is_file() {
                let bytes = fs::read(&path)
                    .map_err(|error| ProviderFailure::Inspect(format!("{source}: {error}")))?;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    if text.contains(".chezmoi.destDir") || text.contains(".chezmoi.targetFile") {
                        return Err(unsupported(&source, "destination_dependent_template"));
                    }
                    for token in [
                        "env",
                        "expandenv",
                        "exec",
                        "output",
                        "lookPath",
                        "stat",
                        "lstat",
                        "glob",
                        "include",
                        "includeTemplate",
                        "httpResponse",
                        "secret",
                        "onepasswordRead",
                        "bitwarden",
                        "keepassxc",
                        "keyring",
                        "pass",
                    ] {
                        if template_invokes(text, token) {
                            return Err(unsupported(&source, "unsafe_template_function"));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_version(&self) -> Result<(), ProviderFailure> {
        let output = Command::new(&self.executable)
            .arg("--version")
            .output()
            .map_err(|error| {
                ProviderFailure::Inspect(format!("could not execute chezmoi: {error}"))
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let reported_version = stdout
            .strip_prefix("chezmoi version ")
            .and_then(|rest| rest.split_whitespace().next())
            .map(|word| word.trim_start_matches('v').trim_end_matches(','));
        if !output.status.success() || reported_version != Some(CHEZMOI_VERSION) {
            return Err(ProviderFailure::Inspect(format!(
                "chezmoi {CHEZMOI_VERSION} is required; install the exact pinned release"
            )));
        }
        Ok(())
    }

    fn inputs(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure> {
        let source_digest = digest_source_tree(&self.source)?;
        let config = fs::read(&self.config).map_err(|error| {
            ProviderFailure::Inspect(format!("{}: {error}", self.config.display()))
        })?;
        let config_digest = digest_domain_json("commonkit.chezmoi-config.v1", &config)
            .map_err(|error| ProviderFailure::Inspect(error.to_string()))?;
        ProviderInputs::new(
            self.id.clone(),
            ExactProviderVersion::parse(CHEZMOI_VERSION)?,
            "commonkit.chezmoi-provider.v1".into(),
            BTreeMap::from([
                ("config".into(), config_digest),
                ("source".into(), source_digest),
                ("targetPlatform".into(), context.platform_facts_digest()?),
            ]),
            vec!["filesystem".into(), "isolated_materialization".into()],
        )
        .map_err(Into::into)
    }
}

impl DesiredStateProvider for ChezmoiProvider {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn inspect_inputs(
        &self,
        context: &ProviderContext,
    ) -> Result<ProviderInputs, ProviderFailure> {
        self.preflight()?;
        self.validate_version()?;
        self.inputs(context)
    }

    fn materialize(
        &self,
        context: &ProviderContext,
        workspace: &ProviderWorkspace,
        artifacts: &ArtifactStore,
    ) -> Result<MaterializedState, ProviderFailure> {
        self.preflight()?;
        self.validate_version()?;
        let inputs = self.inputs(context)?;
        let root = context.declared_roots.first().ok_or_else(|| {
            ProviderFailure::Materialize("chezmoi requires one declared target root".into())
        })?;
        for runtime_path in [&self.cache, &self.persistent_state, &self.working_tree] {
            let resolved = canonicalize_with_missing_leaf(runtime_path).map_err(|error| {
                ProviderFailure::Materialize(format!(
                    "could not validate chezmoi runtime path {}: {error}",
                    runtime_path.display()
                ))
            })?;
            if !resolved.starts_with(workspace.staging_root()) {
                return Err(ProviderFailure::Materialize(format!(
                    "chezmoi runtime path {} must be inside the isolated provider workspace",
                    runtime_path.display()
                )));
            }
        }
        let destination = workspace.staging_root().join("chezmoi-destination");
        if destination.exists() {
            fs::remove_dir_all(&destination).map_err(|error| {
                ProviderFailure::Materialize(format!(
                    "could not reset isolated destination: {error}"
                ))
            })?;
        }
        for directory in [&destination, &self.cache, &self.working_tree] {
            fs::create_dir_all(directory).map_err(|error| {
                ProviderFailure::Materialize(format!(
                    "could not create isolated provider path: {error}"
                ))
            })?;
        }
        let state_parent = self.persistent_state.parent().ok_or_else(|| {
            ProviderFailure::Materialize(
                "chezmoi persistent-state path requires an isolated parent directory".into(),
            )
        })?;
        fs::create_dir_all(state_parent).map_err(|error| {
            ProviderFailure::Materialize(format!(
                "could not create isolated persistent-state parent: {error}"
            ))
        })?;
        let status = Command::new(&self.executable)
            .arg("--source")
            .arg(&self.source)
            .arg("--config")
            .arg(&self.config)
            .arg("--destination")
            .arg(&destination)
            .arg("--cache")
            .arg(&self.cache)
            .arg("--persistent-state")
            .arg(&self.persistent_state)
            .arg("--working-tree")
            .arg(&self.working_tree)
            .arg("--mode=file")
            .arg("--no-tty")
            .arg("--no-pager")
            .arg("--color=off")
            .arg("--os")
            .arg(&context.platform)
            .arg("--arch")
            .arg(&context.architecture)
            .arg("--refresh-externals=never")
            .arg("apply")
            .arg("--force")
            .arg("--exclude=scripts")
            .env_clear()
            .env("HOME", workspace.staging_root())
            .env("TMPDIR", workspace.staging_root())
            .status()
            .map_err(|error| {
                ProviderFailure::Materialize(format!("chezmoi execution failed: {error}"))
            })?;
        if !status.success() {
            return Err(ProviderFailure::Materialize(
                "chezmoi failed while materializing the isolated destination".into(),
            ));
        }
        let resources = scan_destination(&destination, root, &inputs, artifacts)?;
        MaterializedState::finalize(inputs, resources, Vec::new(), Vec::new()).map_err(Into::into)
    }
}

fn unsupported(source: &str, capability: &str) -> ProviderFailure {
    ProviderFailure::Inspect(format!(
        "chezmoi source {source} uses unsupported capability {capability}; remove or migrate that entry before CommonKit planning"
    ))
}

fn template_invokes(text: &str, token: &str) -> bool {
    text.split("{{").skip(1).any(|expression| {
        expression.split("}}").next().is_some_and(|body| {
            body.split(|character: char| character.is_whitespace() || "(|".contains(character))
                .any(|word| word.trim_matches(|c: char| !c.is_alphanumeric()) == token)
        })
    })
}

fn scan_destination(
    destination: &Path,
    root: &NormalizedManagedPath,
    inputs: &ProviderInputs,
    artifacts: &ArtifactStore,
) -> Result<Vec<NormalizedResource>, ProviderFailure> {
    let mut paths = collect_paths(destination)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    paths.sort();
    let mut resources = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(destination).unwrap();
        let portable = portable_path(relative);
        let managed = NormalizedManagedPath::parse(format!("{}/{portable}", root.as_str()))
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| ProviderFailure::Materialize(format!("{portable}: {error}")))?;
        let intent = if metadata.file_type().is_symlink() {
            let target_path = fs::read_link(&path)
                .map_err(|error| ProviderFailure::Materialize(format!("{portable}: {error}")))?;
            let target = target_path.to_str().ok_or_else(|| {
                ProviderFailure::Materialize(format!(
                    "{portable}: symlink target is not portable UTF-8"
                ))
            })?;
            FilesystemIntent::Symlink {
                path: managed.clone(),
                target: SafeSymlinkTarget::parse(&managed, target).map_err(|error| {
                    ProviderFailure::Materialize(format!("{portable}: {error}"))
                })?,
                expected_before: None,
            }
        } else if metadata.is_dir() {
            FilesystemIntent::Directory {
                path: managed,
                mode: mode(&metadata)?,
                exact: false,
            }
        } else if metadata.is_file() {
            let bytes = fs::read(&path)
                .map_err(|error| ProviderFailure::Materialize(format!("{portable}: {error}")))?;
            let content = artifacts
                .put(&bytes, ContentSensitivity::Portable)
                .map_err(|error| ProviderFailure::Materialize(format!("{portable}: {error}")))?;
            FilesystemIntent::File {
                path: managed,
                content,
                mode: mode(&metadata)?,
                expected_before: None,
            }
        } else {
            return Err(ProviderFailure::Materialize(format!(
                "{portable}: staged output has unsupported resource type"
            )));
        };
        resources.push(NormalizedResource {
            intent,
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: CHEZMOI_VERSION.into(),
                input_digest: inputs.input_set_digest.clone(),
                source: format!("chezmoi-stage:{portable}"),
            },
        });
    }
    Ok(resources)
}

#[cfg(unix)]
fn mode(metadata: &fs::Metadata) -> Result<Option<FileMode>, ProviderFailure> {
    use std::os::unix::fs::PermissionsExt;
    FileMode::parse(metadata.permissions().mode() & 0o7777)
        .map(Some)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))
}
#[cfg(not(unix))]
fn mode(_metadata: &fs::Metadata) -> Result<Option<FileMode>, ProviderFailure> {
    Ok(None)
}

fn digest_source_tree(root: &Path) -> Result<Sha256Digest, ProviderFailure> {
    let mut paths =
        collect_paths(root).map_err(|error| ProviderFailure::Inspect(error.to_string()))?;
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        let relative = portable_path(path.strip_prefix(root).unwrap());
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| ProviderFailure::Inspect(format!("{relative}: {error}")))?;
        let value = if metadata.file_type().is_symlink() {
            format!(
                "symlink:{}",
                fs::read_link(&path)
                    .map_err(|error| ProviderFailure::Inspect(error.to_string()))?
                    .display()
            )
        } else if metadata.is_dir() {
            "directory".into()
        } else {
            format!(
                "file:{:?}",
                fs::read(&path).map_err(|error| ProviderFailure::Inspect(error.to_string()))?
            )
        };
        entries.push((relative, value));
    }
    digest_domain_json("commonkit.chezmoi-source.v1", &entries)
        .map_err(|error| ProviderFailure::Inspect(error.to_string()))
}

fn collect_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    fn visit(current: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            output.push(path.clone());
            if entry.file_type()?.is_dir() {
                visit(&path, output)?;
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    visit(root, &mut output)?;
    Ok(output)
}

fn portable_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Canonicalizes the closest existing ancestor, then appends missing ordinary
/// components. This catches symlinked ancestors without requiring the leaf to
/// exist before the provider creates its private runtime directories.
fn canonicalize_with_missing_leaf(path: &Path) -> std::io::Result<PathBuf> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no existing ancestor",
            )
        })?;
        missing.push(name.to_os_string());
        ancestor = ancestor.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no existing ancestor",
            )
        })?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for component in missing.into_iter().rev() {
        if component == "." || component == ".." {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "runtime path contains traversal",
            ));
        }
        resolved.push(component);
    }
    Ok(resolved)
}
