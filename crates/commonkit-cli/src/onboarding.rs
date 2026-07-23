use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, ChezmoiProvider, ContentSensitivity,
    DesiredStateProvider, ExactProviderVersion, FileAdapter, FilesystemIntent, NativeProvider,
    NormalizedManagedPath, NormalizedResource, OwnershipRules, ProviderContext, ProviderInputs,
    ProviderPlanRequest, ProviderWorkspace, ResourceProvenance, build_provider_plan,
};
use commonkit_config::{
    LayerSet, compose_layers, layer_content_digest, v1_merge_rules, validate_layer_content_digest,
};
use commonkit_contracts::{
    LayerDocument, LayerKind, SecurityPolicy, Sha256Digest, StableId, assert_no_embedded_secrets,
    digest_domain_json,
};
use commonkit_core::enforce_policy_floor;
use commonkit_platform::{PrivatePathKind, ensure_private_path};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    /// The personal-kit layer ID. Retains the historical `loadout` name for compatibility.
    pub loadout: String,
    pub project_loadout: Option<String>,
    pub target_override: Option<String>,
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
    let project_loadout = request
        .project_loadout
        .as_deref()
        .map(StableId::parse)
        .transpose()?;
    let target_override = request
        .target_override
        .as_deref()
        .map(StableId::parse)
        .transpose()?;
    let target = StableId::parse(&request.target)?;
    if loadout.as_str() == "public-base" || loadout.as_str() == "organization-policy" {
        return Err(OnboardingError::ReservedLoadout);
    }
    validate_absolute_destination(&request.kit_directory)?;
    validate_absolute_destination(&request.target_root)?;
    validate_absolute_destination(&request.config_directory)?;
    validate_absolute_destination(&request.state_directory)?;
    validate_distinct_roots(request)?;
    if let Some(recovered) = recover_incomplete_onboarding(request, runner)? {
        return Ok(recovered);
    }
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

    let mut selected_layers = vec![
        (StableId::parse("public-base")?, LayerKind::PublicBase),
        (
            StableId::parse("organization-policy")?,
            LayerKind::OrganizationPolicy,
        ),
        (loadout.clone(), LayerKind::PersonalKit),
    ];
    if let Some(id) = project_loadout {
        selected_layers.push((id, LayerKind::ProjectLoadout));
    }
    if let Some(id) = target_override {
        selected_layers.push((id, LayerKind::TargetOverrides));
    }
    let mut selected_ids = BTreeSet::new();
    if selected_layers
        .iter()
        .any(|(id, _)| !selected_ids.insert(id.clone()))
    {
        return Err(OnboardingError::DuplicateLayerSelection);
    }
    let layer_paths = selected_layers
        .iter()
        .map(|(id, _)| {
            request
                .kit_directory
                .join("layers")
                .join(format!("{id}.json"))
        })
        .collect::<Vec<_>>();
    if request.mode == InitMode::Create {
        for ((id, kind), path) in selected_layers.iter().zip(&layer_paths) {
            write_starter_layer(path, id, *kind)?;
        }
        validate_selected_composition(&layer_paths)?;
        write_target_registration(request, &target)?;
        git_commit_created_kit(request, runner)?;
    } else {
        for ((id, kind), path) in selected_layers.iter().zip(&layer_paths) {
            validate_selected_layer(path, id, *kind)?;
        }
        validate_selected_composition(&layer_paths)?;
    }

    let mut revision = git_head_revision(request, runner)?;
    let connect_base_revision = (request.mode == InitMode::Connect).then(|| revision.clone());
    let mut transaction = OnboardingTransaction::begin(
        request,
        connect_base_revision.clone(),
        (request.mode == InitMode::Create).then(|| revision.clone()),
    )?;
    if request.mode == InitMode::Connect {
        // Fully exercise provider materialization and runtime planning before the portable
        // registration is committed. This staged preflight is never made visible locally.
        {
            let preflight = RuntimeStaging::new(request)?;
            write_runtime_state(
                preflight.request(),
                request,
                &provider,
                &layer_paths,
                &revision,
                &target,
            )?;
        }
        write_target_registration(request, &target)?;
        if let Err(error) = git_commit_registration(request, runner) {
            rollback_registration(request, runner, &revision);
            return Err(error);
        }
        let committed_revision = match git_head_revision(request, runner) {
            Ok(revision) => revision,
            Err(error) => {
                rollback_registration(request, runner, &revision);
                return Err(error);
            }
        };
        revision = committed_revision;
        transaction.set_intended_revision(revision.clone())?;
    }

    let mut runtime = match RuntimeStaging::new(request) {
        Ok(runtime) => runtime,
        Err(error) => {
            if let Some(base_revision) = &connect_base_revision {
                rollback_registration(request, runner, base_revision);
            }
            return Err(error);
        }
    };
    if let Err(error) = transaction.set_staging_roots(&runtime) {
        if let Some(base_revision) = &connect_base_revision {
            rollback_registration(request, runner, base_revision);
        }
        return Err(error);
    }
    let runtime_result = write_runtime_state(
        runtime.request(),
        request,
        &provider,
        &layer_paths,
        &revision,
        &target,
    );
    let (_, first_plan_id) = match runtime_result {
        Ok(result) => result,
        Err(error) => {
            if let Some(base_revision) = &connect_base_revision {
                rollback_registration(request, runner, base_revision);
            }
            return Err(error);
        }
    };
    if let Err(error) = transaction.set_runtime(&mut runtime, first_plan_id.clone()) {
        if let Some(base_revision) = &connect_base_revision {
            rollback_registration(request, runner, base_revision);
        }
        return Err(error);
    }
    transaction.preserve_for_recovery();
    if let Err(error) = runtime.publish() {
        let rollback = runtime.rollback();
        if let Some(base_revision) = &connect_base_revision {
            rollback_registration(request, runner, base_revision);
        }
        return match rollback {
            Ok(()) => {
                transaction.remove_after_rollback();
                Err(error)
            }
            Err(rollback_error) => {
                runtime.defer_to_journal();
                Err(rollback_error)
            }
        };
    }
    let push_result = if request.mode == InitMode::Create {
        git_push_created_kit(request, runner)
    } else {
        git_push_registration(request, runner)
    };
    if let Err(error) = push_result {
        let rollback = runtime.rollback();
        if let Some(base_revision) = &connect_base_revision {
            rollback_registration(request, runner, base_revision);
        }
        return match rollback {
            Ok(()) => {
                transaction.remove_after_rollback();
                Err(error)
            }
            Err(rollback_error) => {
                runtime.defer_to_journal();
                Err(rollback_error)
            }
        };
    }
    runtime.commit()?;
    transaction.finish()?;
    let headless_config = request.config_directory.join("headless.json");
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

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct OnboardingTransaction<'a> {
    request: &'a InitRequest,
    journal: OnboardingJournal,
    remove_on_drop: bool,
    created_state_directory: bool,
}

impl<'a> OnboardingTransaction<'a> {
    fn begin(
        request: &'a InitRequest,
        base_revision: Option<String>,
        intended_revision: Option<String>,
    ) -> Result<Self, OnboardingError> {
        let created_state_directory = !request.state_directory.exists();
        let journal = OnboardingJournal {
            schema_version: 1,
            mode: request.mode,
            repository: request.repository.clone(),
            target: request.target.clone(),
            kit_directory: request.kit_directory.clone(),
            base_revision,
            intended_revision,
            first_plan_id: None,
            headless_config: None,
            staging_roots: Vec::new(),
            publications: Vec::new(),
            created_directories: Vec::new(),
        };
        persist_onboarding_journal(request, &journal)?;
        Ok(Self {
            request,
            journal,
            remove_on_drop: true,
            created_state_directory,
        })
    }

    fn set_intended_revision(&mut self, revision: String) -> Result<(), OnboardingError> {
        self.journal.intended_revision = Some(revision);
        persist_onboarding_journal(self.request, &self.journal)
    }

    fn set_staging_roots(&mut self, runtime: &RuntimeStaging) -> Result<(), OnboardingError> {
        self.journal.staging_roots = runtime.staging_roots.clone();
        persist_onboarding_journal(self.request, &self.journal)
    }

    fn set_runtime(
        &mut self,
        runtime: &mut RuntimeStaging,
        first_plan_id: Sha256Digest,
    ) -> Result<(), OnboardingError> {
        let (created_directories, publications) = runtime.prepare_publication()?;
        self.journal.first_plan_id = Some(first_plan_id);
        self.journal.headless_config = Some(self.request.config_directory.join("headless.json"));
        self.journal.staging_roots = runtime.staging_roots.clone();
        self.journal.created_directories = created_directories;
        self.journal.publications = publications;
        persist_onboarding_journal(self.request, &self.journal)
    }

    fn preserve_for_recovery(&mut self) {
        self.remove_on_drop = false;
    }

    fn remove_after_rollback(&mut self) {
        self.remove_on_drop = true;
    }

    fn finish(&mut self) -> Result<(), OnboardingError> {
        remove_onboarding_journal(self.request)?;
        if self.created_state_directory {
            let _ = fs::remove_dir(&self.request.state_directory);
        }
        self.remove_on_drop = false;
        Ok(())
    }
}

impl Drop for OnboardingTransaction<'_> {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = remove_onboarding_journal(self.request);
            if self.created_state_directory {
                let _ = fs::remove_dir(&self.request.state_directory);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalPublication {
    staged: PathBuf,
    destination: PathBuf,
    backup: Option<PathBuf>,
    destination_existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OnboardingJournal {
    schema_version: u32,
    mode: InitMode,
    repository: String,
    target: String,
    kit_directory: PathBuf,
    base_revision: Option<String>,
    intended_revision: Option<String>,
    first_plan_id: Option<Sha256Digest>,
    headless_config: Option<PathBuf>,
    staging_roots: Vec<PathBuf>,
    publications: Vec<JournalPublication>,
    created_directories: Vec<PathBuf>,
}

fn onboarding_journal_path(request: &InitRequest) -> PathBuf {
    request.state_directory.join("onboarding-transaction.json")
}

fn persist_onboarding_journal(
    request: &InitRequest,
    journal: &OnboardingJournal,
) -> Result<(), OnboardingError> {
    let created_state_directories = missing_directory_chain(&request.state_directory);
    ensure_private_path(&request.state_directory, PrivatePathKind::Directory)?;
    sync_created_directory_chain(&created_state_directories, sync_directory)?;
    let path = onboarding_journal_path(request);
    let temporary = request.state_directory.join(format!(
        ".onboarding-transaction.{}.tmp",
        STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    ensure_private_path(&temporary, PrivatePathKind::File)?;
    fs::write(&temporary, serde_json::to_vec_pretty(journal)?)?;
    ensure_private_path(&temporary, PrivatePathKind::File)?;
    fs::File::open(&temporary)?.sync_all()?;
    replace_journal_file(&temporary, &path)?;
    sync_directory(&request.state_directory)?;
    Ok(())
}

fn remove_onboarding_journal(request: &InitRequest) -> Result<(), OnboardingError> {
    let path = onboarding_journal_path(request);
    match fs::remove_file(path) {
        Ok(()) => sync_directory(&request.state_directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path)?.sync_all()
}

fn sync_mutation_parents(
    source: &Path,
    destination: Option<&Path>,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<(), OnboardingError> {
    let source_parent = source
        .parent()
        .ok_or_else(|| OnboardingError::UnsafePath(source.to_owned()))?;
    let destination_parent = destination
        .map(|path| {
            path.parent()
                .ok_or_else(|| OnboardingError::UnsafePath(path.to_owned()))
        })
        .transpose()?;
    let source_result = directory_sync(source_parent);
    let destination_result = if destination_parent.is_some_and(|parent| parent != source_parent) {
        directory_sync(destination_parent.expect("checked as some"))
    } else {
        Ok(())
    };
    source_result?;
    destination_result?;
    Ok(())
}

fn missing_directory_chain(path: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = Some(path);
    while let Some(directory) = current {
        if directory.exists() {
            break;
        }
        missing.push(directory.to_owned());
        current = directory.parent();
    }
    missing
}

fn sync_created_directory_chain(
    missing: &[PathBuf],
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<(), OnboardingError> {
    for directory in missing.iter().rev() {
        sync_mutation_parents(directory, None, directory_sync)?;
    }
    Ok(())
}

fn durable_ensure_directory_tree(
    path: &Path,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<(), OnboardingError> {
    let missing = missing_directory_chain(path);
    fs::create_dir_all(path)?;
    sync_created_directory_chain(&missing, directory_sync)
}

#[cfg(not(windows))]
fn rename_with_write_through(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn rename_with_write_through(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both path buffers remain NUL-terminated and alive for the call. The
    // destination is deliberately not replaced; onboarding prepares unique backups.
    let renamed = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if renamed == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn durable_rename(
    source: &Path,
    destination: &Path,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<(), OnboardingError> {
    rename_with_write_through(source, destination)?;
    sync_mutation_parents(source, Some(destination), directory_sync)
}

fn durable_remove_path(
    path: &Path,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<(), OnboardingError> {
    remove_path(path)?;
    sync_mutation_parents(path, None, directory_sync)
}

fn durable_remove_directory(
    path: &Path,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<(), OnboardingError> {
    match fs::remove_dir(path) {
        Ok(()) => sync_mutation_parents(path, None, directory_sync),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(windows))]
fn replace_journal_file(temporary: &Path, destination: &Path) -> Result<(), std::io::Error> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_journal_file(temporary: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};

    if !destination.exists() {
        return fs::rename(temporary, destination);
    }

    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are NUL-terminated for the duration of the call; the optional
    // backup and metadata pointers are intentionally null. ReplaceFileW atomically swaps
    // an existing journal and requests that Windows flush the replacement before return.
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            temporary.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path)?.sync_all()
}

fn validate_journal_path(path: &Path, allowed_roots: &[&Path]) -> Result<(), OnboardingError> {
    let portable = path.components().all(|component| {
        !matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    });
    if path.is_absolute() && portable && allowed_roots.iter().any(|root| path.starts_with(root)) {
        Ok(())
    } else {
        Err(OnboardingError::InvalidRecoveryJournal)
    }
}

fn recover_incomplete_onboarding(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<Option<InitResult>, OnboardingError> {
    recover_incomplete_onboarding_with_sync(request, runner, sync_directory)
}

fn recover_incomplete_onboarding_with_sync(
    request: &InitRequest,
    runner: &dyn CommandRunner,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
) -> Result<Option<InitResult>, OnboardingError> {
    let path = onboarding_journal_path(request);
    let journal_metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let state_metadata = fs::symlink_metadata(&request.state_directory)?;
    if !state_metadata.is_dir() || state_metadata.file_type().is_symlink() {
        return Err(OnboardingError::InvalidRecoveryJournal);
    }
    if !journal_metadata.is_file() || journal_metadata.file_type().is_symlink() {
        return Err(OnboardingError::InvalidRecoveryJournal);
    }
    let bytes = fs::read(&path)?;
    let journal: OnboardingJournal =
        serde_json::from_slice(&bytes).map_err(|_| OnboardingError::InvalidRecoveryJournal)?;
    if journal.schema_version != 1
        || journal.mode != request.mode
        || journal.repository != request.repository
        || journal.target != request.target
        || journal.kit_directory != request.kit_directory
    {
        return Err(OnboardingError::InvalidRecoveryJournal);
    }
    let allowed_destinations = [
        request.config_directory.as_path(),
        request.state_directory.as_path(),
        request.target_root.as_path(),
    ];
    for staged in &journal.staging_roots {
        let valid_sibling = allowed_destinations.iter().any(|root| {
            let expected_prefix = root
                .file_name()
                .map(|name| format!(".{}.commonkit-stage-", name.to_string_lossy()));
            staged.parent() == root.parent()
                && staged
                    .file_name()
                    .zip(expected_prefix)
                    .is_some_and(|(name, prefix)| name.to_string_lossy().starts_with(&prefix))
        });
        if !valid_sibling {
            return Err(OnboardingError::InvalidRecoveryJournal);
        }
    }
    for publication in &journal.publications {
        if !journal
            .staging_roots
            .iter()
            .any(|root| publication.staged.starts_with(root))
        {
            return Err(OnboardingError::InvalidRecoveryJournal);
        }
        validate_journal_path(&publication.destination, &allowed_destinations)?;
        if let Some(backup) = &publication.backup {
            let expected_prefix = publication
                .destination
                .file_name()
                .map(|name| format!(".{}.commonkit-backup-", name.to_string_lossy()));
            let valid_backup = backup.parent() == publication.destination.parent()
                && backup
                    .file_name()
                    .zip(expected_prefix)
                    .is_some_and(|(name, prefix)| name.to_string_lossy().starts_with(&prefix));
            if !valid_backup {
                return Err(OnboardingError::InvalidRecoveryJournal);
            }
        }
        if publication.destination_existed != publication.backup.is_some() {
            return Err(OnboardingError::InvalidRecoveryJournal);
        }
    }
    for directory in &journal.created_directories {
        validate_journal_path(directory, &allowed_destinations)?;
    }

    let remote_revision = if let Some(intended) = &journal.intended_revision {
        validate_git_revision(intended)?;
        let output = runner.run(
            "git",
            &[
                OsString::from("-C"),
                journal.kit_directory.as_os_str().to_owned(),
                OsString::from("ls-remote"),
                OsString::from("origin"),
                OsString::from("HEAD"),
            ],
        )?;
        let revision = output.split_whitespace().next().map(str::to_owned);
        if let Some(revision) = &revision {
            validate_git_revision(revision)?;
        }
        revision
    } else {
        None
    };

    let remote_matches = journal
        .intended_revision
        .as_ref()
        .is_some_and(|intended| remote_revision.as_ref() == Some(intended));
    if remote_matches {
        let revision = journal
            .intended_revision
            .clone()
            .ok_or(OnboardingError::InvalidRecoveryJournal)?;
        let headless_config = journal
            .headless_config
            .clone()
            .filter(|path| path == &request.config_directory.join("headless.json"))
            .ok_or(OnboardingError::InvalidRecoveryJournal)?;
        let first_plan_id = journal
            .first_plan_id
            .clone()
            .ok_or(OnboardingError::InvalidRecoveryJournal)?;
        for publication in &journal.publications {
            if publication.staged.exists() || !publication.destination.exists() {
                return Err(OnboardingError::MissingRecoveryArtifact(
                    publication.destination.clone(),
                ));
            }
            if let Some(backup) = &publication.backup {
                durable_remove_path(backup, directory_sync)?;
            }
        }
        for staged in &journal.staging_roots {
            durable_remove_path(staged, directory_sync)?;
        }
        remove_onboarding_journal(request)?;
        return Ok(Some(InitResult {
            status: "initialized",
            repository: request.repository.clone(),
            repository_revision: revision,
            kit_directory: request.kit_directory.clone(),
            loadout: request.loadout.clone(),
            target: request.target.clone(),
            headless_config,
            first_plan_id,
        }));
    }

    let definitely_not_pushed = remote_revision.is_none()
        || (journal.mode == InitMode::Connect && remote_revision == journal.base_revision);
    if !definitely_not_pushed {
        return Err(OnboardingError::AmbiguousRemoteRecovery);
    }

    for publication in journal.publications.iter().rev() {
        if let Some(backup) = &publication.backup {
            if backup.exists() {
                if publication.destination.exists() && !publication.staged.exists() {
                    durable_rename(
                        &publication.destination,
                        &publication.staged,
                        directory_sync,
                    )?;
                } else if publication.destination.exists() && publication.staged.exists() {
                    return Err(OnboardingError::ConcurrentPublicationChange(
                        publication.destination.clone(),
                    ));
                }
                durable_rename(backup, &publication.destination, directory_sync)?;
            } else if !publication.staged.exists() || !publication.destination.exists() {
                return Err(OnboardingError::MissingRecoveryArtifact(backup.clone()));
            }
        } else if !publication.staged.exists() && publication.destination.exists() {
            durable_rename(
                &publication.destination,
                &publication.staged,
                directory_sync,
            )?;
        }
    }
    for directory in journal.created_directories.iter().rev() {
        durable_remove_directory(directory, directory_sync)?;
    }
    for staged in &journal.staging_roots {
        durable_remove_path(staged, directory_sync)?;
    }
    if journal.mode == InitMode::Connect {
        if let Some(base_revision) = &journal.base_revision {
            validate_git_revision(base_revision)?;
            rollback_registration(request, runner, base_revision);
        }
    }
    durable_remove_path(&journal.kit_directory, directory_sync)?;
    remove_onboarding_journal(request)?;
    Ok(None)
}

struct RuntimeStaging {
    request: InitRequest,
    staging_roots: Vec<PathBuf>,
    merge_roots: Vec<(PathBuf, PathBuf)>,
    publications: Vec<(PathBuf, PathBuf)>,
    installed: Vec<(PathBuf, PathBuf, Option<PathBuf>)>,
    created_directories: Vec<PathBuf>,
    prepared_publications: Option<(Vec<PathBuf>, Vec<JournalPublication>)>,
    directory_sync: fn(&Path) -> Result<(), std::io::Error>,
    applied: bool,
    committed: bool,
}

impl RuntimeStaging {
    fn new(final_request: &InitRequest) -> Result<Self, OnboardingError> {
        Self::new_with_directory_sync(final_request, sync_directory)
    }

    fn new_with_directory_sync(
        final_request: &InitRequest,
        directory_sync: fn(&Path) -> Result<(), std::io::Error>,
    ) -> Result<Self, OnboardingError> {
        let stage_config = staging_sibling(&final_request.config_directory)?;
        let stage_state = staging_sibling(&final_request.state_directory)?;
        let publications = Vec::new();
        let stage_target = staging_sibling(&final_request.target_root)?;
        let staging_roots = vec![
            stage_config.clone(),
            stage_state.clone(),
            stage_target.clone(),
        ];
        let mut request = final_request.clone();
        request.config_directory = stage_config.clone();
        request.state_directory = stage_state.clone();
        request.target_root = stage_target.clone();
        Ok(Self {
            request,
            staging_roots,
            merge_roots: vec![
                (stage_config, final_request.config_directory.clone()),
                (stage_state, final_request.state_directory.clone()),
                (stage_target, final_request.target_root.clone()),
            ],
            publications,
            installed: Vec::new(),
            created_directories: Vec::new(),
            prepared_publications: None,
            directory_sync,
            applied: false,
            committed: false,
        })
    }

    fn request(&self) -> &InitRequest {
        &self.request
    }

    fn publish(&mut self) -> Result<(), OnboardingError> {
        let (directories, publications) = self
            .prepared_publications
            .clone()
            .map_or_else(|| self.prepare_publication(), Ok)?;
        self.created_directories.clear();
        self.installed = publications
            .iter()
            .map(|publication| {
                (
                    publication.staged.clone(),
                    publication.destination.clone(),
                    publication.backup.clone(),
                )
            })
            .collect();
        self.applied = true;
        for directory in directories {
            match fs::symlink_metadata(&directory) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&directory)?;
                    self.created_directories.push(directory.clone());
                    sync_mutation_parents(&directory, None, self.directory_sync)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        for publication in &publications {
            if !publication.staged.exists() {
                continue;
            }
            if publication.destination.exists() != publication.destination_existed {
                return Err(OnboardingError::ConcurrentPublicationChange(
                    publication.destination.clone(),
                ));
            }
            if let Some(backup) = &publication.backup {
                durable_rename(&publication.destination, backup, self.directory_sync)?;
            }
            durable_rename(
                &publication.staged,
                &publication.destination,
                self.directory_sync,
            )?;
        }
        Ok(())
    }

    fn prepare_publication(
        &mut self,
    ) -> Result<(Vec<PathBuf>, Vec<JournalPublication>), OnboardingError> {
        if let Some(prepared) = &self.prepared_publications {
            return Ok(prepared.clone());
        }
        let mut publications = self.publications.clone();
        let mut directories = Vec::new();
        for (staged, destination) in &self.merge_roots {
            collect_staged_entries(staged, destination, &mut directories, &mut publications)?;
        }
        directories.sort_by_key(|path| path.components().count());
        directories.dedup();
        for directory in &directories {
            if let Ok(metadata) = fs::symlink_metadata(directory)
                && (!metadata.is_dir() || metadata.file_type().is_symlink())
            {
                return Err(OnboardingError::UnsafePath(directory.clone()));
            }
        }
        let created_directories = directories
            .iter()
            .filter(|directory| !directory.exists())
            .cloned()
            .collect::<Vec<_>>();
        let publications = publications
            .into_iter()
            .map(|(staged, destination)| {
                let destination_existed = destination.exists();
                let backup = destination_existed
                    .then(|| backup_sibling(&destination))
                    .transpose()?;
                Ok(JournalPublication {
                    staged,
                    destination,
                    backup,
                    destination_existed,
                })
            })
            .collect::<Result<Vec<_>, OnboardingError>>()?;
        let prepared = (created_directories, publications);
        self.prepared_publications = Some(prepared.clone());
        Ok(prepared)
    }

    fn commit(&mut self) -> Result<(), OnboardingError> {
        self.committed = true;
        for (_, _, backup) in &self.installed {
            if let Some(backup) = backup {
                durable_remove_path(backup, self.directory_sync)?;
            }
        }
        for staged in &self.staging_roots {
            durable_remove_path(staged, self.directory_sync)?;
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), OnboardingError> {
        for (staged_path, installed_path, installed_backup) in self.installed.drain(..).rev() {
            if let Some(installed_backup) = installed_backup {
                if installed_backup.exists() {
                    if installed_path.exists() && !staged_path.exists() {
                        durable_rename(&installed_path, &staged_path, self.directory_sync)?;
                    }
                    durable_rename(&installed_backup, &installed_path, self.directory_sync)?;
                }
            } else if !staged_path.exists() && installed_path.exists() {
                durable_rename(&installed_path, &staged_path, self.directory_sync)?;
            }
        }
        for directory in self.created_directories.drain(..).rev() {
            durable_remove_directory(&directory, self.directory_sync)?;
        }
        self.applied = false;
        Ok(())
    }

    fn defer_to_journal(&mut self) {
        self.committed = true;
    }
}

fn collect_staged_entries(
    staged: &Path,
    destination: &Path,
    directories: &mut Vec<PathBuf>,
    publications: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), OnboardingError> {
    let metadata = fs::symlink_metadata(staged)?;
    if metadata.file_type().is_symlink() {
        return Err(OnboardingError::UnsafePath(staged.into()));
    }
    if metadata.is_file() {
        publications.push((staged.to_owned(), destination.to_owned()));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(OnboardingError::UnsafePath(staged.into()));
    }
    directories.push(destination.to_owned());
    let mut entries = fs::read_dir(staged)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        collect_staged_entries(
            &entry.path(),
            &destination.join(entry.file_name()),
            directories,
            publications,
        )?;
    }
    Ok(())
}

impl Drop for RuntimeStaging {
    fn drop(&mut self) {
        if self.applied && !self.committed {
            let _ = self.rollback();
        }
        if !self.committed {
            for staged in &self.staging_roots {
                let _ = remove_path(staged);
            }
        }
    }
}

fn staging_sibling(destination: &Path) -> Result<PathBuf, OnboardingError> {
    unique_sibling(destination, "stage")
}

fn backup_sibling(destination: &Path) -> Result<PathBuf, OnboardingError> {
    unique_sibling(destination, "backup")
}

fn unique_sibling(destination: &Path, purpose: &str) -> Result<PathBuf, OnboardingError> {
    let parent = destination
        .parent()
        .ok_or_else(|| OnboardingError::UnsafePath(destination.into()))?;
    durable_ensure_directory_tree(parent, sync_directory)?;
    let name = destination
        .file_name()
        .ok_or_else(|| OnboardingError::UnsafePath(destination.into()))?
        .to_string_lossy();
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".{name}.commonkit-{purpose}-{}-{sequence}",
        std::process::id()
    )))
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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

fn git_head_revision(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<String, OnboardingError> {
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
    Ok(revision)
}

fn git_commit_registration(
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

fn git_push_registration(
    request: &InitRequest,
    runner: &dyn CommandRunner,
) -> Result<(), OnboardingError> {
    runner.run(
        "git",
        &[
            OsString::from("-C"),
            request.kit_directory.as_os_str().to_owned(),
            OsString::from("push"),
            OsString::from("origin"),
            OsString::from("HEAD"),
        ],
    )?;
    Ok(())
}

fn rollback_registration(request: &InitRequest, runner: &dyn CommandRunner, base_revision: &str) {
    let _ = runner.run(
        "git",
        &[
            OsString::from("-C"),
            request.kit_directory.as_os_str().to_owned(),
            OsString::from("reset"),
            OsString::from("--hard"),
            OsString::from(base_revision),
        ],
    );
    let _ = fs::remove_file(
        request
            .kit_directory
            .join("targets")
            .join(format!("{}.json", request.target)),
    );
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

fn write_starter_layer(
    path: &Path,
    loadout: &StableId,
    kind: LayerKind,
) -> Result<(), OnboardingError> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| OnboardingError::UnsafePath(path.into()))?,
    )?;
    let mut document: LayerDocument = serde_json::from_value(json!({
        "schemaVersion": 1,
        "id": loadout,
        "kind": kind,
        "source": {
            "path": format!("layers/{loadout}.json"),
            "revision": "0000000000000000000000000000000000000000",
            "contentDigest": format!("sha256:{}", "0".repeat(64)),
        },
        "spec": {},
    }))?;
    document.source.content_digest = layer_content_digest(&document)?;
    write_new_json(path, &document)
}

fn validate_selected_layer(
    path: &Path,
    loadout: &StableId,
    kind: LayerKind,
) -> Result<(), OnboardingError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| OnboardingError::MissingLoadout(path.into()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OnboardingError::MissingLoadout(path.into()));
    }
    let document: LayerDocument = serde_json::from_slice(&fs::read(path)?)?;
    if &document.id != loadout || document.kind != kind {
        return Err(OnboardingError::LoadoutMismatch);
    }
    validate_layer_content_digest(&document)?;
    Ok(())
}

fn validate_selected_composition(layer_paths: &[PathBuf]) -> Result<(), OnboardingError> {
    let mut documents = Vec::with_capacity(layer_paths.len());
    for path in layer_paths {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
        assert_no_embedded_secrets(&value)?;
        let document = serde_json::from_value::<LayerDocument>(value)?;
        validate_layer_content_digest(&document)?;
        documents.push(document);
    }
    let layers = LayerSet::new(documents)?;
    let composition = compose_layers(&layers, &v1_merge_rules())?;
    let organization = layers
        .iter()
        .find(|layer| layer.kind == LayerKind::OrganizationPolicy)
        .and_then(|layer| layer.spec.get("securityPolicy"))
        .map(|value| serde_json::from_value::<SecurityPolicy>(value.clone()))
        .transpose()?
        .unwrap_or_default();
    let effective = composition
        .spec
        .get("securityPolicy")
        .map(|value| serde_json::from_value::<SecurityPolicy>(value.clone()))
        .transpose()?
        .unwrap_or_default();
    enforce_policy_floor(&organization, &effective)?;
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
    let mut registration = json!({
        "schemaVersion": 1,
        "id": target,
        "loadout": request.loadout,
        "transport": "local",
        "managedBy": "commonkit",
    });
    let fields = registration
        .as_object_mut()
        .expect("target registration is an object");
    if let Some(project_loadout) = &request.project_loadout {
        fields.insert("projectLoadout".into(), json!(project_loadout));
    }
    if let Some(target_override) = &request.target_override {
        fields.insert("targetOverride".into(), json!(target_override));
    }
    write_new_json(&path, &registration)
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
    final_request: &InitRequest,
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
                git_executable: None,
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
            "repository": request.repository, "revision": revision, "target": target, "root": final_request.target_root,
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
            "targetRoot": final_request.target_root,
            "adapterState": final_request.state_directory.join("filesystem"),
            "providerArtifacts": final_request.state_directory.join("provider-artifacts"),
            "materializedStates": [],
            "providerPipeline": {
                "root": final_request.state_directory.join("provider-pipeline"),
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
    #[error("each configured composition layer must have a distinct ID")]
    DuplicateLayerSelection,
    #[error("portable registration already exists; review it instead of overwriting: {0}")]
    PortableFileExists(PathBuf),
    #[error("Git returned an invalid HEAD revision")]
    InvalidGitRevision,
    #[error(
        "onboarding recovery journal is invalid; do not mutate managed paths until it is repaired"
    )]
    InvalidRecoveryJournal,
    #[error(
        "remote repository changed while onboarding recovery was pending; inspect the registration before retrying"
    )]
    AmbiguousRemoteRecovery,
    #[error("onboarding recovery artifact is missing or inconsistent: {0}")]
    MissingRecoveryArtifact(PathBuf),
    #[error("managed destination changed after onboarding was planned: {0}")]
    ConcurrentPublicationChange(PathBuf),
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
    LayerSet(#[from] commonkit_config::LayerSetError),
    #[error(transparent)]
    Composition(#[from] commonkit_config::ComposeError),
    #[error(transparent)]
    LayerIntegrity(#[from] commonkit_config::LayerIntegrityError),
    #[error(transparent)]
    Policy(#[from] commonkit_core::PolicyViolation),
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

#[cfg(test)]
mod crash_recovery_tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct RemoteHead(&'static str);

    static SYNCS_UNTIL_FAILURE: AtomicU64 = AtomicU64::new(0);
    static SYNC_FAILURE_TEST: Mutex<()> = Mutex::new(());

    fn fail_requested_directory_sync(path: &Path) -> Result<(), std::io::Error> {
        let remaining = SYNCS_UNTIL_FAILURE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .unwrap_or(0);
        if remaining == 1 {
            Err(std::io::Error::other(format!(
                "injected directory sync failure: {}",
                path.display()
            )))
        } else {
            sync_directory(path)
        }
    }

    impl CommandRunner for RemoteHead {
        fn run(&self, program: &str, arguments: &[OsString]) -> Result<String, OnboardingError> {
            assert_eq!(program, "git");
            if arguments.iter().any(|argument| argument == "ls-remote") {
                return Ok(format!("{}\tHEAD\n", self.0));
            }
            Ok(String::new())
        }
    }

    fn recovery_fixture(root: &Path) -> (InitRequest, OnboardingJournal) {
        let request = InitRequest {
            mode: InitMode::Connect,
            repository: "owner/kit".into(),
            kit_directory: root.join("kit"),
            loadout: "personal".into(),
            project_loadout: None,
            target_override: None,
            target: "workstation".into(),
            target_root: root.join("target"),
            config_directory: root.join("config"),
            state_directory: root.join("state"),
            provider: ProviderSelection::Native,
            publish_registration: true,
        };
        let destination = request.config_directory.join("headless.json");
        let backup = root.join("config/.headless.json.commonkit-backup-1-1");
        let staged = root.join(".config.commonkit-stage-1-1/headless.json");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::create_dir_all(staged.parent().unwrap()).unwrap();
        fs::create_dir_all(&request.kit_directory).unwrap();
        fs::write(&destination, b"new").unwrap();
        fs::write(&backup, b"old").unwrap();
        let journal = OnboardingJournal {
            schema_version: 1,
            mode: InitMode::Connect,
            repository: request.repository.clone(),
            target: request.target.clone(),
            kit_directory: request.kit_directory.clone(),
            base_revision: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            intended_revision: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
            first_plan_id: Some(Sha256Digest::parse(format!("sha256:{}", "1".repeat(64))).unwrap()),
            headless_config: Some(destination.clone()),
            staging_roots: vec![root.join(".config.commonkit-stage-1-1")],
            publications: vec![JournalPublication {
                staged,
                destination,
                backup: Some(backup),
                destination_existed: true,
            }],
            created_directories: vec![],
        };
        (request, journal)
    }

    #[test]
    fn restart_rolls_back_local_publication_when_remote_did_not_accept_registration() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, journal) = recovery_fixture(temporary.path());
        persist_onboarding_journal(&request, &journal).unwrap();

        let recovered = recover_incomplete_onboarding(
            &request,
            &RemoteHead("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap();

        assert!(recovered.is_none());
        assert!(!request.kit_directory.exists());
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"old"
        );
        assert!(!onboarding_journal_path(&request).exists());
    }

    #[test]
    fn restart_rolls_back_a_transaction_interrupted_before_revision_was_recorded() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, mut journal) = recovery_fixture(temporary.path());
        journal.intended_revision = None;
        persist_onboarding_journal(&request, &journal).unwrap();

        let recovered = recover_incomplete_onboarding(
            &request,
            &RemoteHead("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap();

        assert!(recovered.is_none());
        assert!(!request.kit_directory.exists());
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn restart_finalizes_local_publication_when_remote_has_registration() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, journal) = recovery_fixture(temporary.path());
        persist_onboarding_journal(&request, &journal).unwrap();

        let recovered = recover_incomplete_onboarding(
            &request,
            &RemoteHead("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            recovered.repository_revision,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"new"
        );
        assert!(
            !request
                .config_directory
                .join(".headless.json.commonkit-backup-1-1")
                .exists()
        );
        assert!(!onboarding_journal_path(&request).exists());
    }

    #[test]
    fn restart_fails_closed_when_remote_advanced_past_the_recorded_registration() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, journal) = recovery_fixture(temporary.path());
        persist_onboarding_journal(&request, &journal).unwrap();

        let error = recover_incomplete_onboarding(
            &request,
            &RemoteHead("cccccccccccccccccccccccccccccccccccccccc"),
        )
        .unwrap_err();

        assert!(matches!(error, OnboardingError::AmbiguousRemoteRecovery));
        assert!(request.kit_directory.exists());
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"new"
        );
        assert!(onboarding_journal_path(&request).exists());
    }

    #[test]
    fn restart_fails_closed_when_a_required_backup_is_missing() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, journal) = recovery_fixture(temporary.path());
        fs::remove_file(journal.publications[0].backup.as_ref().unwrap()).unwrap();
        persist_onboarding_journal(&request, &journal).unwrap();

        let error = recover_incomplete_onboarding(
            &request,
            &RemoteHead("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap_err();

        assert!(matches!(error, OnboardingError::MissingRecoveryArtifact(_)));
        assert!(request.kit_directory.exists());
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"new"
        );
        assert!(onboarding_journal_path(&request).exists());
    }

    #[test]
    fn publication_sync_failure_keeps_enough_state_for_durable_rollback() {
        let _serial = SYNC_FAILURE_TEST.lock().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let (request, _) = recovery_fixture(temporary.path());
        fs::remove_dir_all(&request.config_directory).unwrap();
        fs::remove_dir_all(&request.kit_directory).unwrap();
        let mut runtime =
            RuntimeStaging::new_with_directory_sync(&request, fail_requested_directory_sync)
                .unwrap();
        for staging_root in &runtime.staging_roots {
            fs::create_dir_all(staging_root).unwrap();
        }
        fs::write(
            runtime.request().config_directory.join("headless.json"),
            b"new",
        )
        .unwrap();
        runtime.prepare_publication().unwrap();
        SYNCS_UNTIL_FAILURE.store(1, Ordering::Relaxed);

        let error = runtime.publish().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected directory sync failure")
        );
        assert!(runtime.applied);
        runtime.rollback().unwrap();
        assert!(!request.config_directory.join("headless.json").exists());
        assert!(!request.config_directory.exists());
    }

    #[test]
    fn backup_rename_sync_failure_restores_the_original_destination() {
        let _serial = SYNC_FAILURE_TEST.lock().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let (request, fixture_journal) = recovery_fixture(temporary.path());
        fs::remove_file(fixture_journal.publications[0].backup.as_ref().unwrap()).unwrap();
        fs::write(request.config_directory.join("headless.json"), b"old").unwrap();
        fs::create_dir_all(&request.state_directory).unwrap();
        fs::create_dir_all(&request.target_root).unwrap();
        let mut runtime =
            RuntimeStaging::new_with_directory_sync(&request, fail_requested_directory_sync)
                .unwrap();
        for staging_root in &runtime.staging_roots {
            fs::create_dir_all(staging_root).unwrap();
        }
        fs::write(
            runtime.request().config_directory.join("headless.json"),
            b"new",
        )
        .unwrap();
        runtime.prepare_publication().unwrap();
        SYNCS_UNTIL_FAILURE.store(1, Ordering::Relaxed);

        let error = runtime.publish().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected directory sync failure")
        );
        runtime.rollback().unwrap();
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"old"
        );
        assert!(runtime.installed.is_empty());
    }

    #[test]
    fn recovery_retry_recognizes_a_restored_backup_after_directory_sync_failure() {
        let _serial = SYNC_FAILURE_TEST.lock().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let (request, journal) = recovery_fixture(temporary.path());
        persist_onboarding_journal(&request, &journal).unwrap();
        // destination -> staging syncs two distinct parents, then backup -> destination
        // syncs their shared parent. Fail after that third rename has taken effect.
        SYNCS_UNTIL_FAILURE.store(3, Ordering::Relaxed);

        let error = recover_incomplete_onboarding_with_sync(
            &request,
            &RemoteHead("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            fail_requested_directory_sync,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected directory sync failure")
        );
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"old"
        );
        assert_eq!(
            fs::read(
                temporary
                    .path()
                    .join(".config.commonkit-stage-1-1/headless.json")
            )
            .unwrap(),
            b"new"
        );
        assert!(
            !request
                .config_directory
                .join(".headless.json.commonkit-backup-1-1")
                .exists()
        );
        assert!(onboarding_journal_path(&request).exists());

        SYNCS_UNTIL_FAILURE.store(0, Ordering::Relaxed);
        let recovered = recover_incomplete_onboarding_with_sync(
            &request,
            &RemoteHead("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            fail_requested_directory_sync,
        )
        .unwrap();

        assert!(recovered.is_none());
        assert_eq!(
            fs::read(request.config_directory.join("headless.json")).unwrap(),
            b"old"
        );
        assert!(!onboarding_journal_path(&request).exists());
        assert!(!request.kit_directory.exists());
    }
}
