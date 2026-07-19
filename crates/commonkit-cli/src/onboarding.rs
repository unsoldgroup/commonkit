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
    pub publish_registration: bool,
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
    if request.mode == InitMode::Connect && !request.publish_registration {
        return Err(OnboardingError::RegistrationConsentRequired);
    }
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
    if request.mode == InitMode::Create {
        validate_create_provider_inputs(&request.provider)?;
    }

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
    let mut checkout_cleanup = (request.mode == InitMode::Create)
        .then(|| CreatedCheckoutCleanup::new(request.kit_directory.clone()));
    let provider = if request.mode == InitMode::Create {
        import_create_provider_inputs(request)?
    } else {
        request.provider.clone()
    };

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
        git_publish_registration(request, runner)?;
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
        write_runtime_state(request, &provider, &layer_paths, &revision, &target)?;
    if request.mode == InitMode::Create {
        git_push_created_kit(request, runner)?;
    }
    let result = InitResult {
        status: "initialized",
        repository: request.repository.clone(),
        repository_revision: revision,
        kit_directory: request.kit_directory.clone(),
        loadout: loadout.to_string(),
        target: target.to_string(),
        headless_config,
        first_plan_id,
    };
    if let Some(cleanup) = checkout_cleanup.as_mut() {
        cleanup.disarm();
    }
    Ok(result)
}

struct CreatedCheckoutCleanup {
    path: PathBuf,
    armed: bool,
}

impl CreatedCheckoutCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CreatedCheckoutCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn git_publish_registration(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<(), OnboardingError> {
    let target = format!("targets/{}.json", request.target);
    for arguments in [
        vec![OsString::from("add"), OsString::from(&target)],
        vec![
            OsString::from("commit"),
            OsString::from("-m"),
            OsString::from(format!("Register CommonKit target {}", request.target)),
        ],
        vec![
            OsString::from("push"),
            OsString::from("origin"),
            OsString::from("HEAD"),
        ],
    ] {
        let combined = [
            OsString::from("-C"),
            request.kit_directory.as_os_str().to_owned(),
        ]
        .into_iter()
        .chain(arguments)
        .collect::<Vec<_>>();
        runner.run("git", &combined)?;
    }
    Ok(())
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
    let mut add = vec![
        OsString::from("add"),
        OsString::from("layers"),
        OsString::from("targets"),
    ];
    if request.kit_directory.join("providers").is_dir() {
        add.push(OsString::from("providers"));
    }
    for arguments in [
        add,
        vec![
            OsString::from("commit"),
            OsString::from("-m"),
            OsString::from("Initialize CommonKit"),
        ],
    ] {
        let combined = prefix.iter().cloned().chain(arguments).collect::<Vec<_>>();
        runner.run("git", &combined)?;
    }
    Ok(())
}

fn git_push_created_kit(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<(), OnboardingError> {
    runner.run(
        "git",
        &[
            OsString::from("-C"),
            request.kit_directory.as_os_str().to_owned(),
            OsString::from("push"),
            OsString::from("--set-upstream"),
            OsString::from("origin"),
            OsString::from("HEAD"),
        ],
    )?;
    Ok(())
}

fn write_runtime_state(
    request: &InitRequest,
    selected_provider: &ProviderSelection,
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
    ) = match selected_provider {
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

fn import_create_provider_inputs(
    request: &InitRequest,
) -> Result<ProviderSelection, OnboardingError> {
    match &request.provider {
        ProviderSelection::Native => Ok(ProviderSelection::Native),
        ProviderSelection::Apm {
            executable,
            manifest,
            lockfile,
            policy,
        } => {
            for path in [manifest, lockfile, policy] {
                if !path.is_absolute() {
                    return Err(OnboardingError::CreateImportMustBeAbsolute(path.clone()));
                }
            }
            let parent = manifest
                .parent()
                .ok_or_else(|| OnboardingError::UnsafePath(manifest.clone()))?;
            if lockfile.parent() != Some(parent) || policy.parent() != Some(parent) {
                return Err(OnboardingError::ApmInputsMustShareDirectory);
            }
            let destination = request.kit_directory.join("providers/apm");
            copy_safe_tree(parent, &destination)?;
            Ok(ProviderSelection::Apm {
                executable: executable.clone(),
                manifest: PathBuf::from("providers/apm").join(
                    manifest
                        .file_name()
                        .ok_or_else(|| OnboardingError::UnsafePath(manifest.clone()))?,
                ),
                lockfile: PathBuf::from("providers/apm").join(
                    lockfile
                        .file_name()
                        .ok_or_else(|| OnboardingError::UnsafePath(lockfile.clone()))?,
                ),
                policy: PathBuf::from("providers/apm").join(
                    policy
                        .file_name()
                        .ok_or_else(|| OnboardingError::UnsafePath(policy.clone()))?,
                ),
            })
        }
        ProviderSelection::Chezmoi {
            executable,
            source,
            config,
        } => {
            if !source.is_absolute() || !config.is_absolute() {
                return Err(OnboardingError::CreateImportMustBeAbsolute(
                    if !source.is_absolute() {
                        source.clone()
                    } else {
                        config.clone()
                    },
                ));
            }
            let destination = request.kit_directory.join("providers/chezmoi/source");
            copy_safe_tree(source, &destination)?;
            let config_bytes = read_safe_import_file(config)?;
            let config_destination = request.kit_directory.join("providers/chezmoi/chezmoi.toml");
            fs::create_dir_all(config_destination.parent().unwrap())?;
            fs::write(&config_destination, &config_bytes)?;
            if fs::read(&config_destination)? != config_bytes {
                return Err(OnboardingError::ImportDigestMismatch);
            }
            Ok(ProviderSelection::Chezmoi {
                executable: executable.clone(),
                source: "providers/chezmoi/source".into(),
                config: "providers/chezmoi/chezmoi.toml".into(),
            })
        }
    }
}

fn validate_create_provider_inputs(provider: &ProviderSelection) -> Result<(), OnboardingError> {
    match provider {
        ProviderSelection::Native => Ok(()),
        ProviderSelection::Apm {
            manifest,
            lockfile,
            policy,
            ..
        } => {
            for path in [manifest, lockfile, policy] {
                if !path.is_absolute() {
                    return Err(OnboardingError::CreateImportMustBeAbsolute(path.clone()));
                }
            }
            let parent = manifest
                .parent()
                .ok_or_else(|| OnboardingError::UnsafePath(manifest.clone()))?;
            if lockfile.parent() != Some(parent) || policy.parent() != Some(parent) {
                return Err(OnboardingError::ApmInputsMustShareDirectory);
            }
            inspect_safe_tree(parent)
        }
        ProviderSelection::Chezmoi { source, config, .. } => {
            if !source.is_absolute() || !config.is_absolute() {
                return Err(OnboardingError::CreateImportMustBeAbsolute(
                    if !source.is_absolute() {
                        source.clone()
                    } else {
                        config.clone()
                    },
                ));
            }
            inspect_safe_tree(source)?;
            read_safe_import_file(config).map(|_| ())
        }
    }
}

fn inspect_safe_tree(source: &Path) -> Result<(), OnboardingError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|_| OnboardingError::MissingProviderInput(source.into()))?;
    if !source.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::UnsafePath(source.into()));
    }
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(OnboardingError::UnsafeImportObject(path));
        }
        if metadata.is_dir() {
            inspect_safe_tree(&path)?;
        } else if metadata.is_file() {
            read_safe_import_file(&path)?;
        } else {
            return Err(OnboardingError::UnsafeImportObject(path));
        }
    }
    Ok(())
}

fn copy_safe_tree(source: &Path, destination: &Path) -> Result<(), OnboardingError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|_| OnboardingError::MissingProviderInput(source.into()))?;
    if !source.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::UnsafePath(source.into()));
    }
    fs::create_dir_all(destination)?;
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let input = entry.path();
        let output = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&input)?;
        if metadata.file_type().is_symlink() {
            return Err(OnboardingError::UnsafeImportObject(input));
        }
        if metadata.is_dir() {
            copy_safe_tree(&input, &output)?;
        } else if metadata.is_file() {
            let bytes = read_safe_import_file(&input)?;
            fs::write(&output, &bytes)?;
            if fs::read(&output)? != bytes {
                return Err(OnboardingError::ImportDigestMismatch);
            }
        } else {
            return Err(OnboardingError::UnsafeImportObject(input));
        }
    }
    Ok(())
}

fn read_safe_import_file(path: &Path) -> Result<Vec<u8>, OnboardingError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| OnboardingError::MissingProviderInput(path.into()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::UnsafeImportObject(path.into()));
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Err(OnboardingError::EmptyProviderInput(path.into()));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| OnboardingError::NonUtf8ProviderInput(path.into()))?;
    {
        commonkit_contracts::assert_no_embedded_secrets(&serde_json::Value::String(
            text.to_owned(),
        ))
        .map_err(|_| OnboardingError::SecretLikeImport(path.into()))?;
        for line in text.lines() {
            if let Some((key, value)) = line.trim().split_once(':') {
                let key = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                let sensitive = [
                    "apikey",
                    "token",
                    "secret",
                    "password",
                    "passwd",
                    "authorization",
                    "credential",
                    "privatekey",
                ]
                .iter()
                .any(|marker| key == *marker || key.ends_with(marker));
                let value = value.trim().trim_matches(['"', '\'']);
                let reference = value.starts_with('$')
                    || ["env:", "secret:", "bws:", "keychain:", "vault:"]
                        .iter()
                        .any(|prefix| value.to_ascii_lowercase().starts_with(prefix));
                if sensitive && !value.is_empty() && value != "false" && !reference {
                    return Err(OnboardingError::SecretLikeImport(path.into()));
                }
            }
        }
    }
    Ok(bytes)
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
            let platform_private_pair = (*left == &request.config_directory
                && *right == &request.state_directory)
                || (*right == &request.config_directory && *left == &request.state_directory);
            if platform_private_pair
                && request
                    .state_directory
                    .starts_with(&request.config_directory)
            {
                continue;
            }
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
    #[error(
        "connecting a target requires explicit --publish-registration consent before CommonKit commits and pushes the portable registration"
    )]
    RegistrationConsentRequired,
    #[error("provider inputs for init create must be absolute import paths: {0}")]
    CreateImportMustBeAbsolute(PathBuf),
    #[error(
        "APM manifest, lockfile, and policy imports must share one directory so manifest-relative sources remain valid"
    )]
    ApmInputsMustShareDirectory,
    #[error("provider import contains a symlink or unsupported filesystem object: {0}")]
    UnsafeImportObject(PathBuf),
    #[error("provider import contains secret-like plaintext: {0}")]
    SecretLikeImport(PathBuf),
    #[error("provider inputs must be ordinary UTF-8 text in v1: {0}")]
    NonUtf8ProviderInput(PathBuf),
    #[error("provider import changed while it was copied; retry from a stable source")]
    ImportDigestMismatch,
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
