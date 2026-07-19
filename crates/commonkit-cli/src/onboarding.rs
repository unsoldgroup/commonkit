use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, ChezmoiProvider, ContentSensitivity,
    DesiredStateProvider, ExactProviderVersion, FileAdapter, FilesystemIntent, NativeProvider,
    NormalizedManagedPath, NormalizedResource, OwnershipRules, ProviderContext, ProviderInputs,
    ProviderPlanRequest, ProviderWorkspace, ResourceProvenance, build_provider_plan,
};
use commonkit_contracts::{LayerDocument, Sha256Digest, StableId, digest_domain_json};
use commonkit_platform::{PrivatePathKind, ensure_private_path};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitMode {
    Create,
    Connect,
}

#[derive(Debug, Clone)]
pub enum ProviderSelection {
    Native,
    Apm {
        executable: PathBuf,
        manifest: PathBuf,
        lockfile: PathBuf,
        policy: PathBuf,
    },
    Chezmoi {
        executable: PathBuf,
        source: PathBuf,
        config: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct InitRequest {
    pub mode: InitMode,
    pub repository: String,
    pub kit_directory: PathBuf,
    pub loadout: String,
    pub target: String,
    pub target_root: PathBuf,
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
    pub provider: ProviderSelection,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitResult {
    pub status: &'static str,
    pub repository: String,
    pub repository_revision: String,
    pub kit_directory: PathBuf,
    pub loadout: String,
    pub target: String,
    pub headless_config: PathBuf,
    pub first_plan_id: Sha256Digest,
}

pub trait CommandRunner {
    fn run(&self, program: &str, arguments: &[OsString]) -> Result<String, OnboardingError>;
}

pub struct ProcessRunner {
    executable_directory: Option<PathBuf>,
}

impl ProcessRunner {
    pub fn from_path() -> Self {
        Self {
            executable_directory: None,
        }
    }

    pub fn new(executable_directory: impl Into<PathBuf>) -> Self {
        Self {
            executable_directory: Some(executable_directory.into()),
        }
    }
}

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &str, arguments: &[OsString]) -> Result<String, OnboardingError> {
        let executable = self.executable_directory.as_ref().map_or_else(
            || PathBuf::from(program),
            |directory| directory.join(program),
        );
        let output = Command::new(executable)
            .args(arguments)
            .output()
            .map_err(|error| OnboardingError::ToolUnavailable {
                tool: program.to_owned(),
                detail: error.to_string(),
            })?;
        if !output.status.success() {
            return Err(OnboardingError::ToolFailed {
                tool: program.to_owned(),
                status: output.status.code(),
            });
        }
        String::from_utf8(output.stdout).map_err(|_| OnboardingError::NonUtf8Output(program.into()))
    }
}

pub fn initialize(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<InitResult, OnboardingError> {
    validate_repository(&request.repository)?;
    let loadout = StableId::parse(&request.loadout)?;
    let target = StableId::parse(&request.target)?;
    if loadout.as_str() == "public-base" || loadout.as_str() == "organization-policy" {
        return Err(OnboardingError::ReservedLoadout);
    }
    validate_absolute_destination(&request.kit_directory)?;
    validate_absolute_destination(&request.target_root)?;
    validate_absolute_destination(&request.config_directory)?;
    validate_absolute_destination(&request.state_directory)?;
    validate_distinct_roots(request)?;
    ensure_clone_destination(&request.kit_directory)?;

    runner.run(
        "gh",
        &os_args(["auth", "status", "--hostname", "github.com"]),
    )?;
    if request.mode == InitMode::Create {
        runner.run(
            "gh",
            &[
                OsString::from("repo"),
                OsString::from("create"),
                OsString::from(&request.repository),
                OsString::from("--private"),
            ],
        )?;
    }
    runner.run(
        "gh",
        &[
            OsString::from("repo"),
            OsString::from("clone"),
            OsString::from(&request.repository),
            request.kit_directory.as_os_str().to_owned(),
        ],
    )?;

    let layer_paths = [
        request.kit_directory.join("layers/public-base.json"),
        request
            .kit_directory
            .join("layers/organization-policy.json"),
        request
            .kit_directory
            .join("layers")
            .join(format!("{loadout}.json")),
    ];
    if request.mode == InitMode::Create {
        write_starter_layer(
            &layer_paths[0],
            &StableId::parse("public-base")?,
            "public_base",
        )?;
        write_starter_layer(
            &layer_paths[1],
            &StableId::parse("organization-policy")?,
            "organization_policy",
        )?;
        write_starter_layer(&layer_paths[2], &loadout, "personal_kit")?;
        write_target_registration(request, &target)?;
        git_commit_created_kit(request, runner)?;
    } else {
        validate_selected_layer(&layer_paths[0], &StableId::parse("public-base")?)?;
        validate_selected_layer(&layer_paths[1], &StableId::parse("organization-policy")?)?;
        validate_selected_layer(&layer_paths[2], &loadout)?;
        write_target_registration(request, &target)?;
    }

    let revision = runner
        .run(
            "git",
            &[
                OsString::from("-C"),
                request.kit_directory.as_os_str().to_owned(),
                OsString::from("rev-parse"),
                OsString::from("HEAD"),
            ],
        )?
        .trim()
        .to_owned();
    validate_git_revision(&revision)?;
    let (headless_config, first_plan_id) =
        write_runtime_state(request, &layer_paths, &revision, &target)?;
    Ok(InitResult {
        status: "initialized",
        repository: request.repository.clone(),
        repository_revision: revision,
        kit_directory: request.kit_directory.clone(),
        loadout: loadout.to_string(),
        target: target.to_string(),
        headless_config,
        first_plan_id,
    })
}

fn validate_repository(repository: &str) -> Result<(), OnboardingError> {
    let mut parts = repository.split('/');
    let valid_part = |value: &str| {
        !value.is_empty()
            && value.len() <= 100
            && !value.starts_with('-')
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    };
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) if valid_part(owner) && valid_part(name) => Ok(()),
        _ => Err(OnboardingError::InvalidRepository),
    }
}

fn validate_absolute_destination(path: &Path) -> Result<(), OnboardingError> {
    if !path.is_absolute() || path.parent().is_none() {
        Err(OnboardingError::UnsafePath(path.to_path_buf()))
    } else {
        Ok(())
    }
}

fn ensure_clone_destination(path: &Path) -> Result<(), OnboardingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(OnboardingError::UnsafePath(path.to_path_buf()))
        }
        Ok(_) if fs::read_dir(path)?.next().is_some() => {
            Err(OnboardingError::DestinationNotEmpty(path.to_path_buf()))
        }
        _ => Ok(()),
    }
}

fn write_starter_layer(path: &Path, loadout: &StableId, kind: &str) -> Result<(), OnboardingError> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| OnboardingError::UnsafePath(path.into()))?,
    )?;
    let document = json!({
        "schemaVersion": 1,
        "id": loadout,
        "kind": kind,
        "source": {
            "path": format!("layers/{loadout}.json"),
            "revision": "0000000000000000000000000000000000000000",
            "contentDigest": digest_domain_json("commonkit.onboarding.layer.v1", &json!({}))?,
        },
        "spec": {},
    });
    write_new_json(path, &document)
}

fn validate_selected_layer(path: &Path, loadout: &StableId) -> Result<(), OnboardingError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| OnboardingError::MissingLoadout(path.into()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::MissingLoadout(path.into()));
    }
    let document: LayerDocument = serde_json::from_slice(&fs::read(path)?)?;
    if &document.id != loadout {
        return Err(OnboardingError::LoadoutMismatch);
    }
    Ok(())
}

fn write_target_registration(
    request: &InitRequest,
    target: &StableId,
) -> Result<(), OnboardingError> {
    let path = request
        .kit_directory
        .join("targets")
        .join(format!("{target}.json"));
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| OnboardingError::UnsafePath(path.clone()))?,
    )?;
    write_new_json(
        &path,
        &json!({
            "schemaVersion": 1,
            "id": target,
            "loadout": request.loadout,
            "transport": "local",
            "managedBy": "commonkit",
        }),
    )
}

fn git_commit_created_kit(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<(), OnboardingError> {
    let prefix = [
        OsString::from("-C"),
        request.kit_directory.as_os_str().to_owned(),
    ];
    for arguments in [
        vec![
            OsString::from("add"),
            OsString::from("layers/public-base.json"),
            OsString::from("layers/organization-policy.json"),
            OsString::from(format!("layers/{}.json", request.loadout)),
            OsString::from(format!("targets/{}.json", request.target)),
        ],
        vec![
            OsString::from("commit"),
            OsString::from("-m"),
            OsString::from("Initialize CommonKit"),
        ],
        vec![
            OsString::from("push"),
            OsString::from("--set-upstream"),
            OsString::from("origin"),
            OsString::from("HEAD"),
        ],
    ] {
        let combined = prefix.iter().cloned().chain(arguments).collect::<Vec<_>>();
        runner.run("git", &combined)?;
    }
    Ok(())
}

fn write_runtime_state(
    request: &InitRequest,
    layer_paths: &[PathBuf],
    revision: &str,
    target: &StableId,
) -> Result<(PathBuf, Sha256Digest), OnboardingError> {
    for directory in [&request.config_directory, &request.state_directory] {
        ensure_private_path(directory, PrivatePathKind::Directory)?;
    }
    ensure_target_root(&request.target_root)?;
    let artifacts = request.state_directory.join("provider-artifacts");
    ensure_private_path(&artifacts, PrivatePathKind::Directory)?;
    let artifact_store = ArtifactStore::open(&artifacts)?;
    let layer_bytes = layer_paths
        .iter()
        .map(fs::read)
        .collect::<Result<Vec<_>, _>>()?;
    let layer_digest = digest_domain_json("commonkit.onboarding.loadout.v1", &layer_bytes)?;
    let inputs = ProviderInputs::new(
        StableId::parse("native")?,
        ExactProviderVersion::parse(env!("CARGO_PKG_VERSION"))?,
        "1".to_owned(),
        BTreeMap::from([
            ("loadout".to_owned(), layer_digest.clone()),
            (
                "repositoryRevision".to_owned(),
                digest_domain_json("commonkit.onboarding.repository-revision.v1", &revision)?,
            ),
        ]),
        vec!["files".to_owned()],
    )?;
    let mut resources = Vec::new();
    for path in layer_paths {
        let document: LayerDocument = serde_json::from_slice(&fs::read(path)?)?;
        let declarations = document
            .spec
            .get("files")
            .cloned()
            .map(serde_json::from_value::<Vec<NativeFileDeclaration>>)
            .transpose()?
            .unwrap_or_default();
        for declaration in declarations {
            let managed_path = NormalizedManagedPath::parse(&declaration.path)?;
            let source_path = NormalizedManagedPath::parse(&declaration.source)?;
            let source = request.kit_directory.join(source_path.as_str());
            let metadata = fs::symlink_metadata(&source)
                .map_err(|_| OnboardingError::MissingProviderSource(declaration.source.clone()))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(OnboardingError::UnsafeProviderSource(declaration.source));
            }
            let canonical_kit = request.kit_directory.canonicalize()?;
            let canonical_source = source.canonicalize()?;
            if !canonical_source.starts_with(&canonical_kit) {
                return Err(OnboardingError::UnsafeProviderSource(declaration.source));
            }
            let content =
                artifact_store.put(&fs::read(&canonical_source)?, ContentSensitivity::Portable)?;
            resources.push((managed_path, content, declaration.source));
        }
    }
    let resources = resources
        .into_iter()
        .map(|(path, content, source)| NormalizedResource {
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
        })
        .collect();
    let native = NativeProvider::new(inputs, resources)?;
    let (provider, provider_config, provider_id): (
        Box<dyn DesiredStateProvider>,
        serde_json::Value,
        &str,
    ) = match &request.provider {
        ProviderSelection::Native => (
            Box::new(native),
            json!({"provider":"native", "version": env!("CARGO_PKG_VERSION"), "files": native_file_configs(layer_paths)?}),
            "native",
        ),
        ProviderSelection::Apm {
            executable,
            manifest,
            lockfile,
            policy,
        } => {
            validate_provider_executable(executable)?;
            let manifest_path = checked_portable_input(&request.kit_directory, manifest)?;
            let lockfile_path = checked_portable_input(&request.kit_directory, lockfile)?;
            let policy_path = checked_portable_input(&request.kit_directory, policy)?;
            let version = ExactProviderVersion::parse("0.25.0")?;
            let provider = ApmProvider::new(ApmProviderConfig {
                executable: executable.clone(),
                version: version.clone(),
                manifest: manifest_path,
                lockfile: lockfile_path,
                policy: policy_path,
                targets: vec!["claude".into(), "codex".into()],
                managed_root: NormalizedManagedPath::parse("agent-context")?,
                bound_source: None,
            })?;
            (
                Box::new(provider),
                json!({
                    "provider":"apm", "executable": executable, "version": version,
                    "manifest": portable_text(manifest)?, "lockfile": portable_text(lockfile)?,
                    "policy": portable_text(policy)?, "targets":["claude","codex"], "managedRoot":"agent-context"
                }),
                "apm",
            )
        }
        ProviderSelection::Chezmoi {
            executable,
            source,
            config,
        } => {
            validate_provider_executable(executable)?;
            let source_path = checked_portable_directory(&request.kit_directory, source)?;
            let config_path = checked_portable_input(&request.kit_directory, config)?;
            let workspace_root = request
                .state_directory
                .join("provider-workspaces/chezmoi/staging");
            let provider = ChezmoiProvider::new(
                executable,
                source_path,
                config_path,
                workspace_root.join("cache"),
                workspace_root.join("state/chezmoi.db"),
                workspace_root.join("working-tree"),
            )?;
            (
                Box::new(provider),
                json!({
                    "provider":"chezmoi", "executable": executable,
                    "source": portable_text(source)?, "config": portable_text(config)?
                }),
                "chezmoi",
            )
        }
    };
    let staging = request
        .state_directory
        .join("provider-workspaces")
        .join(provider_id);
    ensure_private_path(&staging, PrivatePathKind::Directory)?;
    let workspace = ProviderWorkspace::open(
        &staging,
        &[request.target_root.clone(), request.kit_directory.clone()],
    )?;
    let context = ProviderContext {
        target_id: target.clone(),
        platform: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        policy_digest: digest_domain_json(
            "commonkit.onboarding.policy.v1",
            &json!({"organizationFloor": "required"}),
        )?,
        declared_roots: vec![NormalizedManagedPath::parse("portable")?],
        observed_fact_digests: BTreeMap::new(),
    };
    let materialized = provider.materialize(&context, &workspace, &artifact_store)?;
    let provider_state = request
        .state_directory
        .join("providers")
        .join(format!("{provider_id}.json"));
    ensure_private_path(provider_state.parent().unwrap(), PrivatePathKind::Directory)?;
    write_private_json(&provider_state, &materialized)?;

    let adapter_state = request.state_directory.join("filesystem");
    let mut files = FileAdapter::open(&request.target_root, &adapter_state)?;
    let observed = files.observed_state_digest(
        materialized
            .resources
            .iter()
            .map(|resource| &resource.intent),
    )?;
    let ownership = OwnershipRules::new(
        !cfg!(windows),
        vec![
            NormalizedManagedPath::parse("portable")?,
            NormalizedManagedPath::parse("agent-context")?,
        ],
        vec![],
    )?;
    let target_identity_digest = digest_domain_json(
        "commonkit.onboarding.target.v1",
        &json!({
            "repository": request.repository, "revision": revision, "target": target, "root": request.target_root,
        }),
    )?;
    let policy_digest = context.policy_digest.clone();
    let first_plan = build_provider_plan(
        ProviderPlanRequest {
            target_id: target.clone(),
            target_identity_digest: target_identity_digest.clone(),
            composed_loadout_digest: layer_digest.clone(),
            observed_digest: observed,
            policy_digest: policy_digest.clone(),
            ownership_rules: &ownership,
            mapped_side_effects: BTreeSet::new(),
        },
        &[materialized],
        &artifact_store,
        &mut files,
    )?;
    commonkit_reconcile::PlanStore::open(request.state_directory.join("plans"))?
        .persist(&first_plan)?;

    let config = json!({
        "composition": { "layers": layer_paths },
        "sync": {
            "targetId": target,
            "targetRoot": request.target_root,
            "adapterState": adapter_state,
            "providerArtifacts": artifacts,
            "materializedStates": [],
            "providerPipeline": {
                "root": request.state_directory.join("provider-pipeline"),
                "source": {
                    "repository": request.kit_directory,
                    "trustedRemoteUrl": format!("https://github.com/{}.git", request.repository),
                    "revision": revision
                },
                "providers": [provider_config]
            },
            "declaredRoots": ["portable"],
            "protectedRoots": [],
            "caseSensitive": !cfg!(windows),
            "targetIdentityDigest": target_identity_digest,
            "composedLoadoutDigest": layer_digest,
            "policyDigest": policy_digest,
        }
    });
    let path = request.config_directory.join("headless.json");
    write_private_json(&path, &config)?;
    Ok((path, first_plan.id))
}

fn native_file_configs(layer_paths: &[PathBuf]) -> Result<Vec<serde_json::Value>, OnboardingError> {
    let mut files = Vec::new();
    for path in layer_paths {
        let document: LayerDocument = serde_json::from_slice(&fs::read(path)?)?;
        for declaration in document
            .spec
            .get("files")
            .cloned()
            .map(serde_json::from_value::<Vec<NativeFileDeclaration>>)
            .transpose()?
            .unwrap_or_default()
        {
            files.push(json!({"path": declaration.path, "source": declaration.source}));
        }
    }
    Ok(files)
}

fn portable_text(path: &Path) -> Result<String, OnboardingError> {
    Ok(NormalizedManagedPath::parse(
        path.to_str()
            .ok_or_else(|| OnboardingError::UnsafePath(path.into()))?,
    )?
    .to_string())
}

fn checked_portable_input(root: &Path, relative: &Path) -> Result<PathBuf, OnboardingError> {
    let relative = portable_text(relative)?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| OnboardingError::MissingProviderInput(path.clone()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::UnsafePath(path));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root.canonicalize()?) {
        return Err(OnboardingError::UnsafePath(path));
    }
    if fs::read(&canonical)?.is_empty() {
        return Err(OnboardingError::EmptyProviderInput(path));
    }
    Ok(canonical)
}

fn checked_portable_directory(root: &Path, relative: &Path) -> Result<PathBuf, OnboardingError> {
    let relative = portable_text(relative)?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| OnboardingError::MissingProviderInput(path.clone()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::UnsafePath(path));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root.canonicalize()?) {
        return Err(OnboardingError::UnsafePath(path));
    }
    Ok(canonical)
}

fn validate_provider_executable(path: &Path) -> Result<(), OnboardingError> {
    if !path.is_absolute() || !fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
        return Err(OnboardingError::MissingProviderExecutable(path.into()));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeFileDeclaration {
    path: String,
    source: String,
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), OnboardingError> {
    if path.exists() {
        return Err(OnboardingError::PortableFileExists(path.into()));
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), OnboardingError> {
    ensure_private_path(path, PrivatePathKind::File)?;
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    ensure_private_path(path, PrivatePathKind::File)?;
    Ok(())
}

fn validate_git_revision(revision: &str) -> Result<(), OnboardingError> {
    if revision.len() == 40
        && revision
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(OnboardingError::InvalidGitRevision)
    }
}

fn validate_distinct_roots(request: &InitRequest) -> Result<(), OnboardingError> {
    let roots = [
        &request.kit_directory,
        &request.config_directory,
        &request.state_directory,
        &request.target_root,
    ];
    for (index, left) in roots.iter().enumerate() {
        for right in &roots[index + 1..] {
            if left.starts_with(right) || right.starts_with(left) {
                return Err(OnboardingError::OverlappingRoots);
            }
        }
    }
    Ok(())
}

fn ensure_target_root(path: &Path) -> Result<(), OnboardingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(OnboardingError::UnsafePath(path.into()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn os_args<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

#[derive(Debug, Error)]
pub enum OnboardingError {
    #[error("repository must use canonical owner/repository syntax")]
    InvalidRepository,
    #[error("unsafe onboarding path: {0}")]
    UnsafePath(PathBuf),
    #[error("clone destination must be absent or empty: {0}")]
    DestinationNotEmpty(PathBuf),
    #[error("kit, configuration, state, and target roots must not overlap")]
    OverlappingRoots,
    #[error("selected loadout does not exist as a regular layer file: {0}")]
    MissingLoadout(PathBuf),
    #[error("selected loadout ID does not match its layer document")]
    LoadoutMismatch,
    #[error("loadout ID is reserved for a required security layer")]
    ReservedLoadout,
    #[error("portable registration already exists; review it instead of overwriting: {0}")]
    PortableFileExists(PathBuf),
    #[error("Git returned an invalid HEAD revision")]
    InvalidGitRevision,
    #[error("native provider source is missing: {0}")]
    MissingProviderSource(String),
    #[error("native provider source must be a regular non-symlink file: {0}")]
    UnsafeProviderSource(String),
    #[error("provider executable is missing or is not an absolute regular file: {0}")]
    MissingProviderExecutable(PathBuf),
    #[error("provider input is missing: {0}")]
    MissingProviderInput(PathBuf),
    #[error("provider input must not be empty: {0}")]
    EmptyProviderInput(PathBuf),
    #[error("required tool {tool} is unavailable: {detail}")]
    ToolUnavailable { tool: String, detail: String },
    #[error(
        "{tool} failed with status {status:?}; authenticate or resolve the repository error and retry"
    )]
    ToolFailed { tool: String, status: Option<i32> },
    #[error("{0} returned non-UTF-8 output")]
    NonUtf8Output(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error(transparent)]
    Provider(#[from] commonkit_adapters::ProviderContractError),
    #[error(transparent)]
    ProviderFailure(#[from] commonkit_adapters::ProviderFailure),
    #[error(transparent)]
    Resource(#[from] commonkit_adapters::ResourceError),
    #[error(transparent)]
    Artifact(#[from] commonkit_adapters::ArtifactError),
    #[error(transparent)]
    FileAdapter(#[from] commonkit_adapters::FileAdapterError),
    #[error(transparent)]
    Ownership(#[from] commonkit_adapters::OwnershipError),
    #[error(transparent)]
    Plan(#[from] commonkit_adapters::ProviderPlanError),
    #[error(transparent)]
    PlanStore(#[from] commonkit_reconcile::PlanStoreError),
    #[error(transparent)]
    Platform(#[from] commonkit_platform::PlatformError),
}
