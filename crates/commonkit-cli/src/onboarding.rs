use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use commonkit_adapters::{ArtifactStore, ExactProviderVersion, MaterializedState, ProviderInputs};
use commonkit_contracts::{LayerDocument, StableId, digest_domain_json};
use commonkit_platform::{PrivatePathKind, ensure_private_path};
use serde::Serialize;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitMode {
    Create,
    Connect,
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
    let headless_config = write_runtime_state(request, &layer_paths, &revision, &target)?;
    Ok(InitResult {
        status: "initialized",
        repository: request.repository.clone(),
        repository_revision: revision,
        kit_directory: request.kit_directory.clone(),
        loadout: loadout.to_string(),
        target: target.to_string(),
        headless_config,
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
) -> Result<PathBuf, OnboardingError> {
    for directory in [
        &request.config_directory,
        &request.state_directory,
        &request.target_root,
    ] {
        ensure_private_path(directory, PrivatePathKind::Directory)?;
    }
    let artifacts = request.state_directory.join("provider-artifacts");
    ensure_private_path(&artifacts, PrivatePathKind::Directory)?;
    ArtifactStore::open(&artifacts)?;
    let layer_bytes = layer_paths
        .iter()
        .map(fs::read)
        .collect::<Result<Vec<_>, _>>()?;
    let layer_digest = digest_domain_json("commonkit.onboarding.loadout.v1", &layer_bytes)?;
    let inputs = ProviderInputs::new(
        StableId::parse("native")?,
        ExactProviderVersion::parse(env!("CARGO_PKG_VERSION"))?,
        "1".to_owned(),
        BTreeMap::from([("loadout".to_owned(), layer_digest.clone())]),
        vec!["files".to_owned()],
    )?;
    let materialized = MaterializedState::finalize(inputs, vec![], vec![], vec![])?;
    let provider_state = request.state_directory.join("providers/native.json");
    ensure_private_path(provider_state.parent().unwrap(), PrivatePathKind::Directory)?;
    write_private_json(&provider_state, &materialized)?;

    let config = json!({
        "composition": { "layers": layer_paths },
        "sync": {
            "targetId": target,
            "targetRoot": request.target_root,
            "adapterState": request.state_directory.join("filesystem"),
            "providerArtifacts": artifacts,
            "materializedStates": [provider_state],
            "declaredRoots": ["portable"],
            "protectedRoots": [],
            "caseSensitive": !cfg!(windows),
            "targetIdentityDigest": digest_domain_json("commonkit.onboarding.target.v1", &json!({
                "repository": request.repository, "revision": revision, "target": target,
                "root": request.target_root,
            }))?,
            "composedLoadoutDigest": layer_digest,
            "policyDigest": digest_domain_json("commonkit.onboarding.policy.v1", &json!({
                "organizationFloor": "required"
            }))?,
        }
    });
    let path = request.config_directory.join("headless.json");
    write_private_json(&path, &config)?;
    Ok(path)
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
    Artifact(#[from] commonkit_adapters::ArtifactError),
    #[error(transparent)]
    Platform(#[from] commonkit_platform::PlatformError),
}
