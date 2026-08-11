//! Transaction receipts and reconciliation state machine.

mod skill_deployment;

pub use skill_deployment::{
    ApmCompilation, ApmCompiler, AuthenticatedSkillPromotion, CanaryStateObserver,
    DeploymentTrustStore, PreparedSkillDeployment, SkillDeploymentError, SkillDeploymentLineage,
    SkillDeploymentReceipt, SkillDeploymentRecovery, SkillDeploymentRecoveryReceipt,
    SkillDeploymentRequest, SkillDeploymentRollbackFailureReceipt, SkillDeploymentRollbackReceipt,
    SkillDeploymentState, SkillDeploymentWorkflow, SkillPromotionAuthority,
};

#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cap_std::fs::{Dir, OpenOptions as CapOpenOptions};
use commonkit_contracts::{
    CONTRACT_VERSION, ContractError, FORWARD_CONTRACT_VERSION, FORWARD_SCHEMA_VERSION, Operation,
    OperationPhase, OperationProgress, PACKAGE_RECEIPT_CONTRACT_VERSION,
    PACKAGE_RECEIPT_SCHEMA_VERSION, PackageConsent, PackageExitClassification,
    PackageOperationConsentBinding, PackageReceiptAuthorization, PackageReceiptEvidence, Plan,
    PlanBindings, ReceiptState, ReceiptTransition, RecoveryCapability, RunReceipt, SCHEMA_VERSION,
    SchemaVersion, Sha256Digest, StableId, canonical_json, digest_domain_json,
    package_operation_set_digest,
};
use commonkit_core::{PlanBuildError, PlanDraft, build_plan};
use serde::{Deserialize, Serialize};
use thiserror::Error;

static PLAN_TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

pub struct ReceiptJournal {
    receipt: RunReceipt,
}

impl ReceiptJournal {
    pub fn new(
        run_id: StableId,
        plan_id: Sha256Digest,
        target_id: StableId,
        desired_digest: Sha256Digest,
        observed_digest: Sha256Digest,
        policy_digest: Sha256Digest,
        bindings: PlanBindings,
    ) -> Result<Self, ReceiptError> {
        Self::new_versioned(
            run_id,
            plan_id,
            target_id,
            desired_digest,
            observed_digest,
            policy_digest,
            bindings,
            SchemaVersion(SCHEMA_VERSION),
            CONTRACT_VERSION.into(),
            None,
        )
    }

    pub fn for_plan(run_id: StableId, plan: &Plan) -> Result<Self, ReceiptError> {
        Self::new_versioned(
            run_id,
            plan.id.clone(),
            plan.target_id.clone(),
            plan.desired_digest.clone(),
            plan.observed_digest.clone(),
            plan.policy_digest.clone(),
            plan.bindings.clone(),
            plan.schema_version,
            plan.contract_version.clone(),
            None,
        )
    }

    pub fn for_package_plan(
        run_id: StableId,
        plan: &Plan,
        authorization: PackageReceiptAuthorization,
    ) -> Result<Self, ReceiptError> {
        validate_package_receipt_authorization(plan, &authorization)?;
        Self::new_versioned(
            run_id,
            plan.id.clone(),
            plan.target_id.clone(),
            plan.desired_digest.clone(),
            plan.observed_digest.clone(),
            plan.policy_digest.clone(),
            plan.bindings.clone(),
            SchemaVersion(PACKAGE_RECEIPT_SCHEMA_VERSION),
            PACKAGE_RECEIPT_CONTRACT_VERSION.into(),
            Some(authorization),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_versioned(
        run_id: StableId,
        plan_id: Sha256Digest,
        target_id: StableId,
        desired_digest: Sha256Digest,
        observed_digest: Sha256Digest,
        policy_digest: Sha256Digest,
        bindings: PlanBindings,
        schema_version: SchemaVersion,
        contract_version: String,
        package_authorization: Option<PackageReceiptAuthorization>,
    ) -> Result<Self, ReceiptError> {
        validate_receipt_version(schema_version, &contract_version)?;
        let progress = Vec::new();
        validate_package_authorization_shape(schema_version, package_authorization.as_ref())?;
        let progress_digest =
            progress_digest(schema_version, &progress, package_authorization.as_ref())?;
        let first = make_transition_for_schema(
            schema_version,
            0,
            ReceiptState::Prepared,
            None,
            progress_digest,
        )?;
        Ok(Self {
            receipt: RunReceipt {
                schema_version,
                contract_version,
                receipt_id: first.entry_digest.clone(),
                run_id,
                plan_id,
                target_id,
                desired_digest,
                observed_digest,
                policy_digest,
                bindings,
                package_authorization,
                state: ReceiptState::Prepared,
                operation_progress: progress,
                transitions: vec![first],
            },
        })
    }

    pub fn transition(&mut self, next: ReceiptState) -> Result<(), ReceiptError> {
        if !receipt_state_supported(self.receipt.schema_version, next)
            || !legal_transition(self.receipt.state, next)
        {
            return Err(ReceiptError::IllegalTransition {
                from: self.receipt.state,
                to: next,
            });
        }
        let previous = self
            .receipt
            .transitions
            .last()
            .map(|entry| entry.entry_digest.clone());
        let progress_digest = progress_digest(
            self.receipt.schema_version,
            &self.receipt.operation_progress,
            self.receipt.package_authorization.as_ref(),
        )?;
        let transition = make_transition_for_schema(
            self.receipt.schema_version,
            self.receipt.transitions.len() as u64,
            next,
            previous,
            progress_digest,
        )?;
        self.receipt.receipt_id = transition.entry_digest.clone();
        self.receipt.state = next;
        self.receipt.transitions.push(transition);
        Ok(())
    }

    pub fn record_operation(
        &mut self,
        operation_id: Sha256Digest,
        phase: OperationPhase,
        failure_code: Option<StableId>,
    ) -> Result<(), ReceiptError> {
        if !operation_phase_supported(self.receipt.schema_version, phase) {
            return Err(ReceiptError::InvalidOperationProgress);
        }
        if failure_code.is_some()
            != matches!(
                phase,
                OperationPhase::PrepareFailed
                    | OperationPhase::ApplyFailed
                    | OperationPhase::VerifyFailed
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecoveryFailed
            )
        {
            return Err(ReceiptError::InvalidOperationProgress);
        }
        if let Some(progress) = self
            .receipt
            .operation_progress
            .iter_mut()
            .find(|progress| progress.operation_id == operation_id)
        {
            if !legal_operation_transition(progress.phase, phase) {
                return Err(ReceiptError::InvalidOperationProgress);
            }
            progress.phase = phase;
            progress.failure_code = failure_code;
        } else {
            if !matches!(
                phase,
                OperationPhase::Prepared | OperationPhase::PrepareFailed
            ) {
                return Err(ReceiptError::InvalidOperationProgress);
            }
            self.receipt.operation_progress.push(OperationProgress {
                operation_id,
                phase,
                failure_code,
            });
        }
        self.append_progress_transition()
    }

    pub fn record_package_exit(
        &mut self,
        operation_id: &Sha256Digest,
        exit_classification: PackageExitClassification,
        final_digest: Option<Sha256Digest>,
    ) -> Result<(), ReceiptError> {
        let authorization = self
            .receipt
            .package_authorization
            .as_mut()
            .ok_or(ReceiptError::InvalidPackageAuthorization)?;
        let evidence = authorization
            .evidence
            .iter_mut()
            .find(|evidence| &evidence.operation_id == operation_id)
            .ok_or(ReceiptError::InvalidPackageAuthorization)?;
        if matches!(exit_classification, PackageExitClassification::Succeeded)
            != final_digest.is_some()
        {
            return Err(ReceiptError::InvalidPackageAuthorization);
        }
        evidence.exit_classification = exit_classification;
        evidence.final_digest = final_digest;
        self.append_progress_transition()
    }

    fn append_progress_transition(&mut self) -> Result<(), ReceiptError> {
        let previous = self
            .receipt
            .transitions
            .last()
            .map(|entry| entry.entry_digest.clone());
        let progress_digest = progress_digest(
            self.receipt.schema_version,
            &self.receipt.operation_progress,
            self.receipt.package_authorization.as_ref(),
        )?;
        let transition = make_transition_for_schema(
            self.receipt.schema_version,
            self.receipt.transitions.len() as u64,
            self.receipt.state,
            previous,
            progress_digest,
        )?;
        self.receipt.receipt_id = transition.entry_digest.clone();
        self.receipt.transitions.push(transition);
        Ok(())
    }

    pub fn receipt(&self) -> &RunReceipt {
        &self.receipt
    }

    fn from_receipt(receipt: RunReceipt) -> Result<Self, ReceiptError> {
        let journal = Self { receipt };
        journal.verify_chain()?;
        Ok(journal)
    }

    pub fn verify_chain(&self) -> Result<(), ReceiptError> {
        validate_receipt_version(self.receipt.schema_version, &self.receipt.contract_version)?;
        validate_package_authorization_shape(
            self.receipt.schema_version,
            self.receipt.package_authorization.as_ref(),
        )?;
        if self
            .receipt
            .transitions
            .first()
            .is_none_or(|transition| transition.state != ReceiptState::Prepared)
        {
            return Err(ReceiptError::InvalidHashChain);
        }
        let mut previous = None;
        let mut previous_state = None;
        for (sequence, transition) in self.receipt.transitions.iter().enumerate() {
            if !receipt_state_supported(self.receipt.schema_version, transition.state) {
                return Err(ReceiptError::InvalidHashChain);
            }
            if transition.sequence != sequence as u64 || transition.previous_digest != previous {
                return Err(ReceiptError::InvalidHashChain);
            }
            if previous_state.is_some_and(|state| {
                state != transition.state && !legal_transition(state, transition.state)
            }) {
                return Err(ReceiptError::InvalidHashChain);
            }
            let expected = make_transition_for_schema(
                self.receipt.schema_version,
                transition.sequence,
                transition.state,
                previous.clone(),
                transition.progress_digest.clone(),
            )?;
            if expected.entry_digest != transition.entry_digest {
                return Err(ReceiptError::InvalidHashChain);
            }
            previous = Some(transition.entry_digest.clone());
            previous_state = Some(transition.state);
        }
        if previous.as_ref() != Some(&self.receipt.receipt_id) {
            return Err(ReceiptError::InvalidHashChain);
        }
        let progress_digest = progress_digest(
            self.receipt.schema_version,
            &self.receipt.operation_progress,
            self.receipt.package_authorization.as_ref(),
        )?;
        if self
            .receipt
            .operation_progress
            .iter()
            .any(|progress| !operation_phase_supported(self.receipt.schema_version, progress.phase))
        {
            return Err(ReceiptError::InvalidHashChain);
        }
        if self
            .receipt
            .transitions
            .last()
            .is_none_or(|transition| transition.progress_digest != progress_digest)
        {
            return Err(ReceiptError::InvalidHashChain);
        }
        if previous_state != Some(self.receipt.state) {
            return Err(ReceiptError::InvalidHashChain);
        }
        Ok(())
    }
}

/// An immutable, one-snapshot-per-transition receipt store.
///
/// Final files are never replaced. A crash therefore exposes either the prior
/// complete transition or the next complete transition, never a torn update.
pub struct ReceiptStore {
    root: PathBuf,
}

/// Immutable content-addressed storage for approved plans.
pub struct PlanStore {
    root: PathBuf,
    root_handle: Dir,
    root_sync: std::fs::File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanScanResult {
    /// Newest valid plan matching the requested target and bindings.
    pub latest: Option<Plan>,
    /// Whether the scan saw any valid plan for the requested target, including stale bindings.
    pub has_target_candidate: bool,
    /// Whether any known directory entry was malformed or had an invalid plan identity.
    pub has_invalid_candidate: bool,
}

impl PlanStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PlanStoreError> {
        let root = root.as_ref().to_path_buf();
        match fs::symlink_metadata(&root) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || metadata_is_reparse_point(&metadata)
                    || !metadata.is_dir() =>
            {
                return Err(PlanStoreError::InvalidRoot);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&root)?;
            }
            Err(error) => return Err(error.into()),
        }
        set_private_directory(&root)?;
        let (root_handle, root_sync) = open_plan_root(&root)?;
        let root = root.canonicalize()?;
        Ok(Self {
            root,
            root_handle,
            root_sync,
        })
    }

    pub fn persist(&self, plan: &Plan) -> Result<(), PlanStoreError> {
        self.ensure_root()?;
        validate_plan(plan).map_err(|_| PlanStoreError::InvalidPlan)?;
        let bytes = canonical_json(plan)?;
        let destination = self.path(&plan.id);
        match self.root_handle.symlink_metadata(&destination) {
            Ok(_) => {
                return if read_plan_bytes(&self.root_handle, &destination, &self.root)? == bytes {
                    Ok(())
                } else {
                    Err(PlanStoreError::PlanConflict)
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let nonce = PLAN_TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let temporary = PathBuf::from(format!(
            ".plan-{}-{}-{nonce}.tmp",
            std::process::id(),
            plan.id.as_str().trim_start_matches("sha256:")
        ));
        let result = (|| -> Result<(), PlanStoreError> {
            let mut options = CapOpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = self.root_handle.open_with(&temporary, &options)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            match self
                .root_handle
                .hard_link(&temporary, &self.root_handle, &destination)
            {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if read_plan_bytes(&self.root_handle, &destination, &self.root)? != bytes {
                        return Err(PlanStoreError::PlanConflict);
                    }
                }
                Err(error) => return Err(error.into()),
            }
            self.root_handle.remove_file(&temporary)?;
            self.root_sync.sync_all()?;
            self.ensure_root()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.root_handle.remove_file(&temporary);
        }
        result
    }

    pub fn load(&self, id: &Sha256Digest) -> Result<Plan, PlanStoreError> {
        self.ensure_root()?;
        let bytes = read_plan_bytes(&self.root_handle, &self.path(id), &self.root)?;
        let plan: Plan = serde_json::from_slice(&bytes)?;
        if &plan.id != id || validate_plan(&plan).is_err() {
            return Err(PlanStoreError::InvalidPlan);
        }
        Ok(plan)
    }

    /// Loads the most recently persisted valid plan. This is used by offline
    /// drift checks which intentionally do not carry a plan id in their
    /// legacy request shape.
    pub fn load_latest(&self) -> Result<Option<Plan>, PlanStoreError> {
        self.checked_scan(self.load_latest_matching(None, None)?)
    }

    pub fn load_latest_for_target(
        &self,
        target_id: &StableId,
    ) -> Result<Option<Plan>, PlanStoreError> {
        self.checked_scan(self.load_latest_matching(Some(target_id), None)?)
    }

    pub fn load_latest_for_target_with_bindings(
        &self,
        target_id: &StableId,
        target_identity_digest: &Sha256Digest,
        composed_loadout_digest: &Sha256Digest,
        policy_digest: &Sha256Digest,
    ) -> Result<Option<Plan>, PlanStoreError> {
        self.checked_scan(self.load_latest_matching(
            Some(target_id),
            Some((
                target_identity_digest,
                composed_loadout_digest,
                policy_digest,
            )),
        )?)
    }

    /// Scans the plan directory once, returning both the latest matching plan
    /// and evidence needed to distinguish an empty store from stale entries.
    pub fn scan_latest_for_target_with_bindings(
        &self,
        target_id: &StableId,
        target_identity_digest: &Sha256Digest,
        composed_loadout_digest: &Sha256Digest,
        policy_digest: &Sha256Digest,
    ) -> Result<PlanScanResult, PlanStoreError> {
        self.load_latest_matching(
            Some(target_id),
            Some((
                target_identity_digest,
                composed_loadout_digest,
                policy_digest,
            )),
        )
    }

    fn checked_scan(&self, scan: PlanScanResult) -> Result<Option<Plan>, PlanStoreError> {
        if scan.has_invalid_candidate {
            return Err(PlanStoreError::InvalidPlan);
        }
        Ok(scan.latest)
    }

    fn load_latest_matching(
        &self,
        target_id: Option<&StableId>,
        bindings: Option<(&Sha256Digest, &Sha256Digest, &Sha256Digest)>,
    ) -> Result<PlanScanResult, PlanStoreError> {
        self.ensure_root()?;
        let mut latest: Option<(std::time::SystemTime, Sha256Digest, Plan)> = None;
        let mut has_target_candidate = false;
        let mut has_invalid_candidate = false;
        for entry in self.root_handle.entries()? {
            let entry = entry?;
            let name_os = entry.file_name();
            let Some(name) = name_os.to_str() else {
                has_invalid_candidate = true;
                continue;
            };
            if is_allowed_plan_store_entry(name) {
                continue;
            }
            if Path::new(name).extension().and_then(|value| value.to_str()) != Some("json") {
                has_invalid_candidate = true;
                continue;
            }
            let (bytes, modified) = read_plan_file(&self.root_handle, Path::new(name), &self.root)?;
            let plan: Plan = match serde_json::from_slice(&bytes) {
                Ok(plan) => plan,
                Err(_) => {
                    has_invalid_candidate = true;
                    continue;
                }
            };
            if validate_plan(&plan).is_err() {
                has_invalid_candidate = true;
                continue;
            }
            let expected_name = format!("{}.json", plan.id.as_str().trim_start_matches("sha256:"));
            if name != expected_name {
                has_invalid_candidate = true;
                continue;
            }
            if target_id.is_some_and(|target| target != &plan.target_id) {
                continue;
            }
            has_target_candidate = true;
            if bindings.is_some_and(|(target, loadout, policy)| {
                plan.bindings.target_identity_digest != *target
                    || plan.bindings.composed_loadout_digest != *loadout
                    || plan.policy_digest != *policy
            }) {
                continue;
            }
            if latest.as_ref().is_none_or(|(current, current_id, _)| {
                modified > *current || (modified == *current && plan.id > *current_id)
            }) {
                latest = Some((modified, plan.id.clone(), plan));
            }
        }
        self.ensure_root()?;
        Ok(PlanScanResult {
            latest: latest.map(|(_, _, plan)| plan),
            has_target_candidate,
            has_invalid_candidate,
        })
    }

    fn ensure_root(&self) -> Result<(), PlanStoreError> {
        let (_, current) = open_plan_root(&self.root)?;
        if !same_directory(&self.root_sync, &current)? {
            return Err(PlanStoreError::InvalidRoot);
        }
        Ok(())
    }

    fn path(&self, id: &Sha256Digest) -> PathBuf {
        PathBuf::from(format!(
            "{}.json",
            id.as_str().trim_start_matches("sha256:")
        ))
    }
}

fn is_allowed_plan_store_entry(name: &str) -> bool {
    if name == ".plan-store.metadata" {
        return true;
    }
    let Some(value) = name
        .strip_prefix(".plan-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    let mut parts = value.split('-');
    let Some(pid) = parts.next() else {
        return false;
    };
    let Some(digest) = parts.next() else {
        return false;
    };
    let Some(nonce) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !nonce.is_empty()
        && nonce.bytes().all(|byte| byte.is_ascii_digit())
}

fn open_plan_root(path: &Path) -> Result<(Dir, std::fs::File), PlanStoreError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.access_mode(GENERIC_READ | GENERIC_WRITE);
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        if no_follow_open_error(&error) {
            PlanStoreError::InvalidRoot
        } else {
            error.into()
        }
    })?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata_is_reparse_point(&metadata) {
        return Err(PlanStoreError::InvalidRoot);
    }
    let root_sync = file.try_clone()?;
    Ok((Dir::from_std_file(file), root_sync))
}

fn read_plan_bytes(
    directory: &Dir,
    name: &Path,
    root_path: &Path,
) -> Result<Vec<u8>, PlanStoreError> {
    let (bytes, _) = read_plan_file(directory, name, root_path)?;
    Ok(bytes)
}

fn read_plan_file(
    directory: &Dir,
    name: &Path,
    root_path: &Path,
) -> Result<(Vec<u8>, std::time::SystemTime), PlanStoreError> {
    let (mut file, metadata) = open_plan_file(directory, name, root_path)?;
    let modified = metadata
        .modified()
        .map(|time| time.into_std())
        .unwrap_or(std::time::UNIX_EPOCH);
    #[cfg(windows)]
    let opened_identity = windows_cap_file_identity(&file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    #[cfg(windows)]
    {
        let (current_file, _) = open_plan_file(directory, name, root_path)?;
        if opened_identity != windows_cap_file_identity(&current_file)? {
            return Err(PlanStoreError::InvalidPlan);
        }
    }
    #[cfg(not(windows))]
    {
        let current = directory.symlink_metadata(name)?;
        if current.file_type().is_symlink()
            || !current.is_file()
            || !same_cap_file(&metadata, &current)
        {
            return Err(PlanStoreError::InvalidPlan);
        }
    }
    Ok((bytes, modified))
}

#[cfg(test)]
fn open_plan_file_from_path(
    path: &Path,
) -> Result<(std::fs::File, std::fs::Metadata), PlanStoreError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        if no_follow_open_error(&error) {
            PlanStoreError::UnsafeEntry
        } else {
            error.into()
        }
    })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse_point(&metadata) {
        return Err(PlanStoreError::UnsafeEntry);
    }
    Ok((file, metadata))
}

fn open_plan_file(
    directory: &Dir,
    name: &Path,
    root_path: &Path,
) -> Result<(cap_std::fs::File, cap_std::fs::Metadata), PlanStoreError> {
    #[cfg(unix)]
    {
        let _ = root_path;
        let root = directory.try_clone()?.into_std_file();
        let file = rustix::fs::openat(
            &root,
            name,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map(std::fs::File::from)
        .map_err(|error| {
            let error: std::io::Error = error.into();
            if no_follow_open_error(&error) {
                PlanStoreError::UnsafeEntry
            } else {
                error.into()
            }
        })?;
        let file = cap_std::fs::File::from_std(file);
        let metadata = file.metadata()?;
        if !metadata.is_file() || cap_metadata_is_reparse_point(&metadata) {
            return Err(PlanStoreError::UnsafeEntry);
        }
        Ok((file, metadata))
    }
    #[cfg(windows)]
    {
        open_plan_file_windows(directory, name, root_path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let mut options = CapOpenOptions::new();
        options.read(true);
        let file = directory.open_with(name, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || cap_metadata_is_reparse_point(&metadata) {
            return Err(PlanStoreError::UnsafeEntry);
        }
        Ok((file, metadata))
    }
}

#[cfg(windows)]
fn open_plan_file_windows(
    directory: &Dir,
    name: &Path,
    root_path: &Path,
) -> Result<(cap_std::fs::File, cap_std::fs::Metadata), PlanStoreError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let path = root_path.join(name);
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    // SAFETY: `wide` is NUL-terminated and remains alive for the call. The
    // returned handle is owned and converted immediately when valid.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `handle` is a valid owned handle from CreateFileW.
    let file = unsafe { std::fs::File::from_raw_handle(handle) };
    let file = cap_std::fs::File::from_std(file);
    let metadata = file.metadata()?;
    if !metadata.is_file() || cap_metadata_is_reparse_point(&metadata) {
        return Err(PlanStoreError::UnsafeEntry);
    }
    let _ = directory;
    Ok((file, metadata))
}

fn no_follow_open_error(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn same_cap_file(opened: &cap_std::fs::Metadata, current: &cap_std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        opened.dev() == current.dev() && opened.ino() == current.ino()
    }
    #[cfg(not(unix))]
    {
        opened.len() == current.len() && opened.modified().ok() == current.modified().ok()
    }
}

#[cfg(windows)]
fn windows_cap_file_identity(file: &cap_std::fs::File) -> Result<(u32, u64), PlanStoreError> {
    let file = file.try_clone()?.into_std();
    Ok(windows_file_identity(&file)?)
}

fn same_directory(opened: &std::fs::File, current: &std::fs::File) -> Result<bool, std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = opened.metadata()?;
        let current = current.metadata()?;
        Ok(opened.dev() == current.dev() && opened.ino() == current.ino())
    }
    #[cfg(windows)]
    {
        Ok(windows_file_identity(opened)? == windows_file_identity(current)?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(opened.metadata()?.modified().ok() == current.metadata()?.modified().ok())
    }
}

#[cfg(windows)]
fn windows_file_identity(file: &std::fs::File) -> Result<(u32, u64), std::io::Error> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the live handle is valid and Windows initializes the output on success.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful GetFileInformationByHandle initialized the structure.
    let information = unsafe { information.assume_init() };
    Ok((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn cap_metadata_is_reparse_point(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn cap_metadata_is_reparse_point(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum PlanStoreError {
    #[error("plan store root is invalid")]
    InvalidRoot,
    #[error("plan store entry is not an ordinary file")]
    UnsafeEntry,
    #[error("plan is invalid or does not match its content address")]
    InvalidPlan,
    #[error("plan ID is already bound to different content")]
    PlanConflict,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] ContractError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFailure {
    pub code: String,
    pub message: String,
}

impl AdapterFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub trait Adapter {
    fn id(&self) -> &StableId;
    fn supports_recovery(&self, _capability: RecoveryCapability) -> bool {
        false
    }
    fn supports_operation(&self, operation: &Operation) -> bool {
        self.supports_recovery(operation.recovery_capability)
    }
    /// Read-only execution preflight. This runs before any receipt is created.
    fn preflight(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    /// Returns consent binding and sanitized before-evidence for package operations.
    fn package_authorization_binding(
        &self,
        _operation: &Operation,
    ) -> Result<Option<(PackageOperationConsentBinding, PackageReceiptEvidence)>, AdapterFailure>
    {
        Ok(None)
    }
    /// Returns sanitized final package-state evidence for the v3 receipt.
    fn package_final_digest(
        &mut self,
        _operation: &Operation,
    ) -> Result<Option<Sha256Digest>, AdapterFailure> {
        Ok(None)
    }
    fn observe_recovery(
        &mut self,
        _operation: &Operation,
    ) -> Result<RecoveryObservation, AdapterFailure> {
        Err(AdapterFailure::new(
            "recovery_observation_unsupported",
            "adapter does not expose recovery observation",
        ))
    }
    fn supports_offline_recovery(&self, _operation: &Operation) -> bool {
        false
    }
    fn prepare_recovery(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Err(AdapterFailure::new(
            "offline_recovery_unsupported",
            "adapter does not expose offline recovery preparation",
        ))
    }
    fn converge_recovery(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Err(AdapterFailure::new(
            "offline_recovery_unsupported",
            "adapter does not expose offline recovery convergence",
        ))
    }
    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryObservation {
    Before,
    After,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileOutcome {
    Succeeded,
    Canceled,
    RolledBack,
    RollbackFailed,
    ForwardRecoveryRequired,
    ForwardRecovered,
    ForwardRecoveryFailed,
}

impl ReconcileOutcome {
    pub fn receipt_state(self) -> ReceiptState {
        match self {
            Self::Succeeded => ReceiptState::Succeeded,
            Self::Canceled => ReceiptState::Canceled,
            Self::RolledBack => ReceiptState::RolledBack,
            Self::RollbackFailed => ReceiptState::RollbackFailed,
            Self::ForwardRecoveryRequired => ReceiptState::ForwardRecoveryRequired,
            Self::ForwardRecovered => ReceiptState::ForwardRecovered,
            Self::ForwardRecoveryFailed => ReceiptState::ForwardRecoveryFailed,
        }
    }
}

#[derive(Default)]
pub struct Reconciler<'a> {
    store: Option<&'a ReceiptStore>,
}

impl<'a> Reconciler<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_store(store: &'a ReceiptStore) -> Self {
        Self { store: Some(store) }
    }

    pub fn execute(
        &self,
        plan: &Plan,
        run_id: StableId,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        if plan.operations.iter().any(is_package_operation) {
            return Err(ReconcileError::PackageConsentRequired);
        }
        preflight_adapters(plan, adapters)?;
        let journal = ReceiptJournal::for_plan(run_id, plan)?;
        self.execute_preflighted(plan, adapters, journal)
    }

    /// Validates the reviewed package operation set without opening an adapter,
    /// touching a target, or creating a receipt. The package adapter repeats
    /// this binding against its persisted resolution before execution.
    pub fn validate_package_consent(
        plan: &Plan,
        consent: &PackageConsent,
    ) -> Result<(), ReconcileError> {
        validate_plan(plan)?;
        let bindings = package_bindings_from_plan(plan)?;
        consent
            .validate(plan, &bindings)
            .map_err(|_| ReconcileError::PackageConsentMismatch)?;
        Ok(())
    }

    pub fn execute_with_package_consent(
        &self,
        plan: &Plan,
        run_id: StableId,
        consent: &PackageConsent,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        // This first pass is deliberately adapter-free. A malformed or stale
        // consent must not cause target observation, transport setup, or a
        // receipt checkpoint before it is rejected.
        let expected_bindings = package_bindings_from_plan(plan)?;
        consent
            .validate(plan, &expected_bindings)
            .map_err(|_| ReconcileError::PackageConsentMismatch)?;
        preflight_adapters(plan, adapters)?;
        let mut bindings = Vec::new();
        let mut evidence = Vec::new();
        for operation in plan
            .operations
            .iter()
            .filter(|operation| is_package_operation(operation))
        {
            let adapter = adapters
                .iter()
                .find(|adapter| adapter.id() == &operation.adapter_id)
                .ok_or_else(|| ReconcileError::AdapterNotFound(operation.adapter_id.clone()))?;
            let (binding, operation_evidence) = adapter
                .package_authorization_binding(operation)
                .map_err(ReconcileError::AdapterPreflightFailed)?
                .ok_or_else(|| ReconcileError::PackageAuthorizationUnavailable {
                    adapter_id: operation.adapter_id.clone(),
                })?;
            bindings.push(binding);
            evidence.push(operation_evidence);
        }
        if bindings != expected_bindings {
            return Err(ReconcileError::PackageConsentBindingChanged);
        }
        consent
            .validate(plan, &bindings)
            .map_err(|_| ReconcileError::PackageConsentMismatch)?;
        let authorization = PackageReceiptAuthorization {
            consent_digest: consent.digest().map_err(ReceiptError::from)?,
            confirmation_id: consent.confirmation_id.clone(),
            operation_set_digest: consent.operation_set_digest.clone(),
            evidence,
        };
        let journal = ReceiptJournal::for_package_plan(run_id, plan, authorization)?;
        self.execute_preflighted(plan, adapters, journal)
    }

    fn execute_preflighted(
        &self,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
        mut journal: ReceiptJournal,
    ) -> Result<ReconcileOutcome, ReconcileError> {
        self.persist(&journal)?;

        for operation in &plan.operations {
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.prepare(operation) {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::Prepared,
                    None,
                )?,
                Err(failure) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::PrepareFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                    self.persist(&journal)?;
                    record_package_failure(&mut journal, operation, &failure)?;
                    self.persist(&journal)?;
                    journal.transition(ReceiptState::Canceled)?;
                    self.persist(&journal)?;
                    return Ok(ReconcileOutcome::Canceled);
                }
            }
            self.persist(&journal)?;
        }

        journal.transition(ReceiptState::Applying)?;
        self.persist(&journal)?;
        let exact = plan
            .operations
            .iter()
            .filter(|operation| operation.recovery_capability == RecoveryCapability::ExactRollback)
            .collect::<Vec<_>>();
        let forward = plan
            .operations
            .iter()
            .filter(|operation| {
                operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly
            })
            .collect::<Vec<_>>();
        let mut applied = Vec::new();
        for operation in &exact {
            journal.record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)?;
            self.persist(&journal)?;
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.apply(operation) {
                Ok(()) => {
                    journal.record_operation(operation.id.clone(), OperationPhase::Applied, None)?
                }
                Err(failure) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::ApplyFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                    self.persist(&journal)?;
                    record_package_failure(&mut journal, operation, &failure)?;
                    self.persist(&journal)?;
                    applied.push((*operation).clone());
                    return self.recover(&mut journal, &applied, adapters);
                }
            }
            applied.push((*operation).clone());
            self.persist(&journal)?;
        }
        journal.transition(ReceiptState::Verifying)?;
        self.persist(&journal)?;
        for operation in &exact {
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.verify(operation) {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::Verified,
                    None,
                )?,
                Err(failure) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::VerifyFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                    self.persist(&journal)?;
                    record_package_failure(&mut journal, operation, &failure)?;
                    self.persist(&journal)?;
                    return self.recover(&mut journal, &applied, adapters);
                }
            }
            self.persist(&journal)?;
        }

        if !forward.is_empty() {
            journal.transition(ReceiptState::ApplyingForward)?;
            self.persist(&journal)?;
        }
        for operation in forward {
            // This durable checkpoint is the one-way recovery barrier. No
            // rollback path is reachable after it has been persisted.
            journal.record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)?;
            self.persist(&journal)?;
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            if let Err(failure) = adapter.apply(operation) {
                journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::ApplyFailed,
                    Some(stable_failure_code(&failure)),
                )?;
                self.persist(&journal)?;
                record_package_failure(&mut journal, operation, &failure)?;
                self.persist(&journal)?;
                return self.require_forward_recovery(&mut journal);
            }
            journal.record_operation(operation.id.clone(), OperationPhase::Applied, None)?;
            self.persist(&journal)?;
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            if let Err(failure) = adapter.verify(operation) {
                journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::VerifyFailed,
                    Some(stable_failure_code(&failure)),
                )?;
                self.persist(&journal)?;
                record_package_failure(&mut journal, operation, &failure)?;
                self.persist(&journal)?;
                return self.require_forward_recovery(&mut journal);
            }
            journal.record_operation(operation.id.clone(), OperationPhase::Verified, None)?;
            self.persist(&journal)?;
            record_package_success(&mut journal, operation, adapters)?;
            self.persist(&journal)?;
        }

        journal.transition(ReceiptState::Succeeded)?;
        self.persist(&journal)?;
        Ok(ReconcileOutcome::Succeeded)
    }

    pub fn recover_run(
        &self,
        run_id: StableId,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        preflight_adapters(plan, adapters)?;
        let store = self.store.ok_or(ReconcileError::DurableStoreRequired)?;
        let mut journal = store.load(run_id)?;
        let receipt = journal.receipt();
        validate_receipt_plan_binding(plan, receipt)?;

        let after_forward_barrier = plan.operations.iter().any(|operation| {
            operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly
                && receipt.operation_progress.iter().any(|progress| {
                    progress.operation_id == operation.id
                        && matches!(
                            progress.phase,
                            OperationPhase::ApplyStarted
                                | OperationPhase::Applied
                                | OperationPhase::ApplyFailed
                                | OperationPhase::Verified
                                | OperationPhase::VerifyFailed
                                | OperationPhase::ForwardRecovered
                                | OperationPhase::ForwardRecoveryFailed
                        )
                })
        }) || matches!(
            receipt.state,
            ReceiptState::ForwardRecoveryRequired
                | ReceiptState::ConvergingForward
                | ReceiptState::ForwardRecoveryFailed
        );
        if after_forward_barrier {
            return self.converge_forward(&mut journal, plan, adapters);
        }

        match receipt.state {
            ReceiptState::Prepared => {
                journal.transition(ReceiptState::Canceled)?;
                self.persist(&journal)?;
                return Ok(ReconcileOutcome::Canceled);
            }
            ReceiptState::Applying | ReceiptState::Verifying | ReceiptState::ApplyingForward => {
                journal.transition(ReceiptState::RecoveryRequired)?;
                self.persist(&journal)?;
            }
            ReceiptState::RecoveryRequired => {}
            ReceiptState::RollingBack => {}
            state => return Err(ReconcileError::RunAlreadyTerminal(state)),
        }
        if journal.receipt().state == ReceiptState::RecoveryRequired {
            journal.transition(ReceiptState::RollingBack)?;
            self.persist(&journal)?;
        }

        let progress = journal.receipt().operation_progress.clone();
        let mut applied = Vec::new();
        for operation in &plan.operations {
            let phase = progress
                .iter()
                .find(|entry| entry.operation_id == operation.id)
                .map(|entry| entry.phase);
            if matches!(
                phase,
                Some(
                    OperationPhase::ApplyStarted
                        | OperationPhase::Applied
                        | OperationPhase::ApplyFailed
                        | OperationPhase::Verified
                        | OperationPhase::VerifyFailed
                )
            ) {
                applied.push(operation.clone());
            }
        }
        self.rollback_from_rolling_back(&mut journal, &applied, adapters)
    }

    /// Explicitly rolls back a previously successful durable run without
    /// re-resolving provider inputs or secret material.
    pub fn rollback_succeeded_run(
        &self,
        run_id: StableId,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        validate_plan(plan)?;
        if plan.operations.iter().any(|operation| {
            operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly
        }) {
            return Err(ReconcileError::RollbackUnsupported);
        }
        preflight_adapters(plan, adapters)?;
        let store = self.store.ok_or(ReconcileError::DurableStoreRequired)?;
        let mut journal = store.load(run_id)?;
        if journal.receipt().plan_id != plan.id
            || journal.receipt().schema_version != plan.schema_version
            || journal.receipt().contract_version != plan.contract_version
            || journal.receipt().target_id != plan.target_id
        {
            return Err(ReconcileError::ReceiptPlanMismatch);
        }
        if journal.receipt().state != ReceiptState::Succeeded {
            return Err(ReconcileError::RunAlreadyTerminal(journal.receipt().state));
        }
        journal.transition(ReceiptState::RollingBack)?;
        self.persist(&journal)?;
        self.rollback_from_rolling_back(&mut journal, &plan.operations, adapters)
    }

    fn recover(
        &self,
        journal: &mut ReceiptJournal,
        applied: &[Operation],
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        journal.transition(ReceiptState::RecoveryRequired)?;
        self.persist(journal)?;
        journal.transition(ReceiptState::RollingBack)?;
        self.persist(journal)?;

        self.rollback_from_rolling_back(journal, applied, adapters)
    }

    fn require_forward_recovery(
        &self,
        journal: &mut ReceiptJournal,
    ) -> Result<ReconcileOutcome, ReconcileError> {
        journal.transition(ReceiptState::ForwardRecoveryRequired)?;
        self.persist(journal)?;
        Ok(ReconcileOutcome::ForwardRecoveryRequired)
    }

    fn converge_forward(
        &self,
        journal: &mut ReceiptJournal,
        plan: &Plan,
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        match journal.receipt().state {
            ReceiptState::Applying | ReceiptState::Verifying | ReceiptState::ApplyingForward => {
                journal.transition(ReceiptState::ForwardRecoveryRequired)?;
                self.persist(journal)?;
            }
            ReceiptState::ForwardRecoveryRequired => {}
            ReceiptState::ConvergingForward => {}
            ReceiptState::ForwardRecoveryFailed => {}
            state => return Err(ReconcileError::RunAlreadyTerminal(state)),
        }
        if matches!(
            journal.receipt().state,
            ReceiptState::ForwardRecoveryRequired | ReceiptState::ForwardRecoveryFailed
        ) {
            journal.transition(ReceiptState::ConvergingForward)?;
            self.persist(journal)?;
        }

        for operation in &plan.operations {
            let already_recovered = journal.receipt().operation_progress.iter().any(|progress| {
                progress.operation_id == operation.id
                    && progress.phase == OperationPhase::ForwardRecovered
            });
            if already_recovered {
                if package_evidence_pending(journal, operation) {
                    record_package_success(journal, operation, adapters)?;
                    self.persist(journal)?;
                }
                continue;
            }
            let observation = adapter_for(adapters, &operation.adapter_id)?
                .observe_recovery(operation)
                .map_err(|failure| stable_failure_code(&failure));
            let result = match observation {
                Ok(RecoveryObservation::After) => Ok(()),
                Ok(RecoveryObservation::Before) => {
                    let prepare =
                        adapter_for(adapters, &operation.adapter_id)?.prepare_recovery(operation);
                    if let Err(failure) = prepare {
                        Err(stable_failure_code(&failure))
                    } else {
                        let converge = adapter_for(adapters, &operation.adapter_id)?
                            .converge_recovery(operation);
                        if let Err(failure) = converge {
                            Err(stable_failure_code(&failure))
                        } else {
                            match adapter_for(adapters, &operation.adapter_id)?
                                .observe_recovery(operation)
                            {
                                Ok(RecoveryObservation::After) => Ok(()),
                                Ok(RecoveryObservation::Before | RecoveryObservation::Other) => {
                                    Err(StableId::parse("recovery_ambiguous")
                                        .expect("static stable ID"))
                                }
                                Err(failure) => Err(stable_failure_code(&failure)),
                            }
                        }
                    }
                }
                Ok(RecoveryObservation::Other) => {
                    Err(StableId::parse("recovery_ambiguous").expect("static stable ID"))
                }
                Err(code) => Err(code),
            };
            match result {
                Ok(()) => {
                    // Package evidence must be durably present before the
                    // operation can be marked ForwardRecovered. Recovery
                    // still repairs receipts written by the old ordering.
                    record_package_success(journal, operation, adapters)?;
                    self.persist(journal)?;
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::ForwardRecovered,
                        None,
                    )?;
                    self.persist(journal)?;
                }
                Err(code) => {
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::ForwardRecoveryFailed,
                        Some(code.clone()),
                    )?;
                    self.persist(journal)?;
                    record_package_failure_code(journal, operation, code)?;
                    self.persist(journal)?;
                    journal.transition(ReceiptState::ForwardRecoveryFailed)?;
                    self.persist(journal)?;
                    return Ok(ReconcileOutcome::ForwardRecoveryFailed);
                }
            }
            self.persist(journal)?;
        }
        journal.transition(ReceiptState::ForwardRecovered)?;
        self.persist(journal)?;
        Ok(ReconcileOutcome::ForwardRecovered)
    }

    fn rollback_from_rolling_back(
        &self,
        journal: &mut ReceiptJournal,
        applied: &[Operation],
        adapters: &mut [Box<dyn Adapter>],
    ) -> Result<ReconcileOutcome, ReconcileError> {
        let mut rollback_failed = false;
        for operation in applied.iter().rev() {
            let adapter = adapter_for(adapters, &operation.adapter_id)?;
            match adapter.rollback(operation) {
                Ok(()) => journal.record_operation(
                    operation.id.clone(),
                    OperationPhase::RolledBack,
                    None,
                )?,
                Err(failure) => {
                    rollback_failed = true;
                    journal.record_operation(
                        operation.id.clone(),
                        OperationPhase::RollbackFailed,
                        Some(stable_failure_code(&failure)),
                    )?;
                }
            }
            self.persist(journal)?;
        }
        let outcome = if rollback_failed {
            ReconcileOutcome::RollbackFailed
        } else {
            ReconcileOutcome::RolledBack
        };
        journal.transition(outcome.receipt_state())?;
        self.persist(journal)?;
        Ok(outcome)
    }

    fn persist(&self, journal: &ReceiptJournal) -> Result<(), ReconcileError> {
        if let Some(store) = self.store {
            store.persist(journal)?;
        }
        Ok(())
    }
}

fn record_package_failure(
    journal: &mut ReceiptJournal,
    operation: &Operation,
    failure: &AdapterFailure,
) -> Result<(), ReceiptError> {
    record_package_failure_code(journal, operation, stable_failure_code(failure))
}

fn record_package_failure_code(
    journal: &mut ReceiptJournal,
    operation: &Operation,
    code: StableId,
) -> Result<(), ReceiptError> {
    if is_package_operation(operation) && journal.receipt().package_authorization.is_some() {
        journal.record_package_exit(
            &operation.id,
            PackageExitClassification::Failed { code },
            None,
        )?;
    }
    Ok(())
}

fn record_package_success(
    journal: &mut ReceiptJournal,
    operation: &Operation,
    adapters: &mut [Box<dyn Adapter>],
) -> Result<(), ReconcileError> {
    if !is_package_operation(operation) || journal.receipt().package_authorization.is_none() {
        return Ok(());
    }
    let digest = adapter_for(adapters, &operation.adapter_id)?
        .package_final_digest(operation)
        .map_err(ReconcileError::AdapterPreflightFailed)?
        .ok_or_else(|| ReconcileError::PackageEvidenceUnavailable {
            adapter_id: operation.adapter_id.clone(),
        })?;
    journal.record_package_exit(
        &operation.id,
        PackageExitClassification::Succeeded,
        Some(digest),
    )?;
    Ok(())
}

fn package_evidence_pending(journal: &ReceiptJournal, operation: &Operation) -> bool {
    journal
        .receipt()
        .package_authorization
        .as_ref()
        .is_some_and(|authorization| {
            authorization.evidence.iter().any(|evidence| {
                evidence.operation_id == operation.id
                    && !matches!(
                        evidence.exit_classification,
                        PackageExitClassification::Succeeded
                    )
            })
        })
}

fn stable_failure_code(failure: &AdapterFailure) -> StableId {
    StableId::parse(failure.code.clone())
        .unwrap_or_else(|_| StableId::parse("adapter_failure").expect("static stable ID"))
}

fn adapter_for<'a>(
    adapters: &'a mut [Box<dyn Adapter>],
    id: &StableId,
) -> Result<&'a mut (dyn Adapter + 'a), ReconcileError> {
    for adapter in adapters {
        if adapter.id() == id {
            return Ok(adapter.as_mut());
        }
    }
    Err(ReconcileError::AdapterNotFound(id.clone()))
}

fn preflight_adapters(
    plan: &Plan,
    adapters: &mut [Box<dyn Adapter>],
) -> Result<(), ReconcileError> {
    let crosses_forward_barrier = plan
        .operations
        .iter()
        .any(|operation| operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly);
    for operation in &plan.operations {
        let adapter = adapters
            .iter_mut()
            .find(|adapter| adapter.id() == &operation.adapter_id)
            .ok_or_else(|| ReconcileError::AdapterNotFound(operation.adapter_id.clone()))?;
        if !adapter.supports_operation(operation) {
            return Err(ReconcileError::AdapterCapabilityUnsupported {
                adapter_id: operation.adapter_id.clone(),
                capability: operation.recovery_capability,
            });
        }
        if crosses_forward_barrier && !adapter.supports_offline_recovery(operation) {
            return Err(ReconcileError::AdapterOfflineRecoveryUnsupported {
                adapter_id: operation.adapter_id.clone(),
            });
        }
        adapter
            .preflight(operation)
            .map_err(ReconcileError::AdapterPreflightFailed)?;
    }
    Ok(())
}

fn is_package_operation(operation: &Operation) -> bool {
    operation.adapter_id.as_str() == "packages"
        || operation.resource.resource_type.as_str() == "package"
}

fn package_bindings_from_plan(
    plan: &Plan,
) -> Result<Vec<PackageOperationConsentBinding>, ReconcileError> {
    let package_operations = plan
        .operations
        .iter()
        .filter(|operation| is_package_operation(operation))
        .map(|operation| PackageOperationConsentBinding {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
        })
        .collect::<Vec<_>>();
    if package_operations.is_empty() {
        return Err(ReconcileError::PackageConsentRequired);
    }
    if plan.bindings.package_resolution_authority_digest.is_none() {
        return Err(ReconcileError::PackageResolutionAuthorityMissing);
    }
    Ok(package_operations)
}

fn validate_plan(plan: &Plan) -> Result<(), ReconcileError> {
    let rebuilt = build_plan(PlanDraft {
        target_id: plan.target_id.clone(),
        desired_digest: plan.desired_digest.clone(),
        observed_digest: plan.observed_digest.clone(),
        policy_digest: plan.policy_digest.clone(),
        bindings: plan.bindings.clone(),
        operations: plan.operations.clone(),
    })?;
    if rebuilt != *plan {
        return Err(ReconcileError::PlanMismatch);
    }
    Ok(())
}

/// Binds a durable receipt to its plan while keeping the package receipt
/// version upgrade explicit. Forward plans are v2; package authorization adds
/// v3 receipt evidence without changing the plan identity or its bindings.
fn validate_receipt_plan_binding(plan: &Plan, receipt: &RunReceipt) -> Result<(), ReconcileError> {
    if receipt.plan_id != plan.id
        || receipt.target_id != plan.target_id
        || receipt.desired_digest != plan.desired_digest
        || receipt.observed_digest != plan.observed_digest
        || receipt.policy_digest != plan.policy_digest
        || receipt.bindings != plan.bindings
    {
        return Err(ReconcileError::ReceiptPlanMismatch);
    }

    let exact_version = receipt.schema_version == plan.schema_version
        && receipt.contract_version == plan.contract_version;
    let package_receipt_upgrade = plan.schema_version.0 == FORWARD_SCHEMA_VERSION
        && plan.contract_version == FORWARD_CONTRACT_VERSION
        && receipt.schema_version.0 == PACKAGE_RECEIPT_SCHEMA_VERSION
        && receipt.contract_version == PACKAGE_RECEIPT_CONTRACT_VERSION
        && receipt.package_authorization.is_some();
    if !exact_version && !package_receipt_upgrade {
        return Err(ReconcileError::ReceiptPlanMismatch);
    }

    if package_receipt_upgrade {
        let authorization = receipt
            .package_authorization
            .as_ref()
            .ok_or(ReconcileError::ReceiptPlanMismatch)?;
        let expected_bindings =
            package_bindings_from_plan(plan).map_err(|_| ReconcileError::ReceiptPlanMismatch)?;
        let receipt_bindings = authorization
            .evidence
            .iter()
            .map(|evidence| PackageOperationConsentBinding {
                operation_id: evidence.operation_id.clone(),
                resolution_digest: evidence.resolution_digest.clone(),
            })
            .collect::<Vec<_>>();
        if receipt_bindings != expected_bindings
            || authorization.operation_set_digest
                != package_operation_set_digest(plan, &receipt_bindings)
                    .map_err(|_| ReconcileError::ReceiptPlanMismatch)?
        {
            return Err(ReconcileError::ReceiptPlanMismatch);
        }
        validate_package_receipt_authorization(plan, authorization)
            .map_err(|_| ReconcileError::ReceiptPlanMismatch)?;
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error(transparent)]
    Plan(#[from] PlanBuildError),
    #[error("plan content does not match its content-addressed identity")]
    PlanMismatch,
    #[error("package operations require explicit package consent")]
    PackageConsentRequired,
    #[error("package consent does not match the reviewed operation set")]
    PackageConsentMismatch,
    #[error("package plan is missing package-resolution authority")]
    PackageResolutionAuthorityMissing,
    #[error("package authorization evidence is unavailable from adapter {adapter_id}")]
    PackageAuthorizationUnavailable { adapter_id: StableId },
    #[error("package adapter changed the consent binding before execution")]
    PackageConsentBindingChanged,
    #[error("package adapter {adapter_id} did not provide final-state evidence")]
    PackageEvidenceUnavailable { adapter_id: StableId },
    #[error("adapter preflight failed: {0:?}")]
    AdapterPreflightFailed(AdapterFailure),
    #[error("adapter is not registered: {0}")]
    AdapterNotFound(StableId),
    #[error("adapter {adapter_id} does not support recovery capability {capability:?}")]
    AdapterCapabilityUnsupported {
        adapter_id: StableId,
        capability: RecoveryCapability,
    },
    #[error("adapter {adapter_id} does not support bound offline forward recovery")]
    AdapterOfflineRecoveryUnsupported { adapter_id: StableId },
    #[error("explicit rollback is unsupported for a forward-only plan")]
    RollbackUnsupported,
    #[error("durable receipt store is required for restart recovery")]
    DurableStoreRequired,
    #[error("receipt is not bound to the supplied plan")]
    ReceiptPlanMismatch,
    #[error("run is already terminal in state {0:?}")]
    RunAlreadyTerminal(ReceiptState),
}

impl ReceiptStore {
    /// Root of the capability-scoped durable receipt store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self, ReceiptError> {
        fs::create_dir_all(root.as_ref())?;
        sync_directory(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }

    /// Returns every durably recorded run identifier in stable order.
    ///
    /// Invalid directory names fail closed: callers must not silently skip a
    /// receipt that may require recovery.
    pub fn run_ids(&self) -> Result<Vec<StableId>, ReceiptError> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(ReceiptError::InvalidRunDirectory);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ReceiptError::InvalidRunDirectory)?;
            ids.push(StableId::parse(name).map_err(|_| ReceiptError::InvalidRunDirectory)?);
        }
        ids.sort();
        Ok(ids)
    }

    pub fn persist(&self, journal: &ReceiptJournal) -> Result<(), ReceiptError> {
        journal.verify_chain()?;
        let receipt = journal.receipt();
        let run_directory = self.root.join(receipt.run_id.as_str());
        fs::create_dir_all(&run_directory)?;

        if let Ok(current) = self.load(receipt.run_id.clone()) {
            let current_count = current.receipt().transitions.len();
            let incoming_count = receipt.transitions.len();
            if current_count >= incoming_count {
                if current.receipt() == receipt {
                    return Ok(());
                }
                return Err(ReceiptError::StaleWrite {
                    persisted_sequence: current_count.saturating_sub(1) as u64,
                    attempted_sequence: incoming_count.saturating_sub(1) as u64,
                });
            }
            if current.receipt().transitions != receipt.transitions[..current_count] {
                return Err(ReceiptError::ConflictingHistory);
            }
        }

        let sequence = receipt.transitions.len().saturating_sub(1) as u64;
        let digest = receipt
            .transitions
            .last()
            .ok_or(ReceiptError::InvalidHashChain)?
            .entry_digest
            .as_str()
            .trim_start_matches("sha256:");
        let final_path = run_directory.join(format!("{sequence:020}-{digest}.json"));
        if final_path.exists() {
            return Ok(());
        }
        let temporary_path = run_directory.join(format!(".{sequence:020}-{digest}.tmp"));
        let bytes = canonical_json(receipt)?;
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        fs::rename(&temporary_path, &final_path)?;
        sync_directory(&run_directory)?;
        Ok(())
    }

    pub fn load(&self, run_id: StableId) -> Result<ReceiptJournal, ReceiptError> {
        let run_directory = self.root.join(run_id.as_str());
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(&run_directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or(ReceiptError::InvalidSnapshotName)?;
            let sequence = name
                .split_once('-')
                .and_then(|(value, _)| value.parse::<u64>().ok())
                .ok_or(ReceiptError::InvalidSnapshotName)?;
            snapshots.push((sequence, path));
        }
        snapshots.sort_by_key(|(sequence, _)| *sequence);
        if snapshots.is_empty() {
            return Err(ReceiptError::NotFound(run_id));
        }

        let mut previous: Option<RunReceipt> = None;
        for (expected, (sequence, path)) in snapshots.into_iter().enumerate() {
            if sequence != expected as u64 {
                return Err(ReceiptError::SnapshotGap);
            }
            let receipt: RunReceipt = serde_json::from_slice(&fs::read(path)?)?;
            if receipt.run_id != run_id || receipt.transitions.len() != expected + 1 {
                return Err(ReceiptError::ConflictingHistory);
            }
            ReceiptJournal::from_receipt(receipt.clone())?;
            if let Some(prior) = &previous
                && prior.transitions != receipt.transitions[..prior.transitions.len()]
            {
                return Err(ReceiptError::ConflictingHistory);
            }
            previous = Some(receipt);
        }
        ReceiptJournal::from_receipt(previous.expect("non-empty snapshots"))
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransitionSemantic<'a> {
    sequence: u64,
    state: ReceiptState,
    previous_digest: &'a Option<Sha256Digest>,
    progress_digest: &'a Sha256Digest,
}

#[cfg(test)]
fn make_transition(
    sequence: u64,
    state: ReceiptState,
    previous_digest: Option<Sha256Digest>,
    progress_digest: Sha256Digest,
) -> Result<ReceiptTransition, ContractError> {
    make_transition_for_schema(
        SchemaVersion(SCHEMA_VERSION),
        sequence,
        state,
        previous_digest,
        progress_digest,
    )
}

fn make_transition_for_schema(
    schema_version: SchemaVersion,
    sequence: u64,
    state: ReceiptState,
    previous_digest: Option<Sha256Digest>,
    progress_digest: Sha256Digest,
) -> Result<ReceiptTransition, ContractError> {
    let entry_digest = digest_domain_json(
        match schema_version.0 {
            PACKAGE_RECEIPT_SCHEMA_VERSION => "commonkit.receipt-transition.v3",
            FORWARD_SCHEMA_VERSION => "commonkit.receipt-transition.v2",
            _ => "commonkit.receipt-transition.v1",
        },
        &TransitionSemantic {
            sequence,
            state,
            previous_digest: &previous_digest,
            progress_digest: &progress_digest,
        },
    )?;
    Ok(ReceiptTransition {
        sequence,
        state,
        previous_digest,
        progress_digest,
        entry_digest,
    })
}

fn progress_digest(
    schema_version: SchemaVersion,
    progress: &[OperationProgress],
    package_authorization: Option<&PackageReceiptAuthorization>,
) -> Result<Sha256Digest, ContractError> {
    if schema_version.0 == PACKAGE_RECEIPT_SCHEMA_VERSION {
        return digest_domain_json(
            "commonkit.operation-progress.v3",
            &(progress, package_authorization),
        );
    }
    digest_domain_json(
        if schema_version.0 == FORWARD_SCHEMA_VERSION {
            "commonkit.operation-progress.v2"
        } else {
            "commonkit.operation-progress.v1"
        },
        progress,
    )
}

fn validate_receipt_version(
    schema_version: SchemaVersion,
    contract_version: &str,
) -> Result<(), ReceiptError> {
    if (schema_version.0 == SCHEMA_VERSION && contract_version == CONTRACT_VERSION)
        || (schema_version.0 == FORWARD_SCHEMA_VERSION
            && contract_version == FORWARD_CONTRACT_VERSION)
        || (schema_version.0 == PACKAGE_RECEIPT_SCHEMA_VERSION
            && contract_version == PACKAGE_RECEIPT_CONTRACT_VERSION)
    {
        Ok(())
    } else {
        Err(ReceiptError::UnsupportedSchemaVersion(schema_version.0))
    }
}

fn receipt_state_supported(schema_version: SchemaVersion, state: ReceiptState) -> bool {
    schema_version.0 >= FORWARD_SCHEMA_VERSION
        || !matches!(
            state,
            ReceiptState::ApplyingForward
                | ReceiptState::ForwardRecoveryRequired
                | ReceiptState::ConvergingForward
                | ReceiptState::ForwardRecovered
                | ReceiptState::ForwardRecoveryFailed
        )
}

fn operation_phase_supported(schema_version: SchemaVersion, phase: OperationPhase) -> bool {
    schema_version.0 >= FORWARD_SCHEMA_VERSION
        || !matches!(
            phase,
            OperationPhase::ForwardRecovered | OperationPhase::ForwardRecoveryFailed
        )
}

fn validate_package_authorization_shape(
    schema_version: SchemaVersion,
    authorization: Option<&PackageReceiptAuthorization>,
) -> Result<(), ReceiptError> {
    if schema_version.0 != PACKAGE_RECEIPT_SCHEMA_VERSION {
        return if authorization.is_none() {
            Ok(())
        } else {
            Err(ReceiptError::InvalidPackageAuthorization)
        };
    }
    let authorization = authorization.ok_or(ReceiptError::InvalidPackageAuthorization)?;
    if authorization.evidence.is_empty() {
        return Err(ReceiptError::InvalidPackageAuthorization);
    }
    let mut operation_ids = std::collections::BTreeSet::new();
    for evidence in &authorization.evidence {
        if !operation_ids.insert(&evidence.operation_id) {
            return Err(ReceiptError::InvalidPackageAuthorization);
        }
        match (&evidence.exit_classification, &evidence.final_digest) {
            (PackageExitClassification::Succeeded, Some(_))
            | (PackageExitClassification::NotRun, None)
            | (PackageExitClassification::Failed { .. }, None) => {}
            _ => return Err(ReceiptError::InvalidPackageAuthorization),
        }
    }
    Ok(())
}

fn validate_package_receipt_authorization(
    plan: &Plan,
    authorization: &PackageReceiptAuthorization,
) -> Result<(), ReceiptError> {
    validate_package_authorization_shape(
        SchemaVersion(PACKAGE_RECEIPT_SCHEMA_VERSION),
        Some(authorization),
    )?;
    let package_operations = plan
        .operations
        .iter()
        .filter(|operation| {
            operation.adapter_id.as_str() == "packages"
                || operation.resource.resource_type.as_str() == "package"
        })
        .collect::<Vec<_>>();
    if package_operations.len() != authorization.evidence.len()
        || package_operations.iter().any(|operation| {
            !authorization
                .evidence
                .iter()
                .any(|evidence| evidence.operation_id == operation.id)
        })
    {
        return Err(ReceiptError::InvalidPackageAuthorization);
    }
    Ok(())
}

fn legal_operation_transition(from: OperationPhase, to: OperationPhase) -> bool {
    matches!(
        (from, to),
        (OperationPhase::Prepared, OperationPhase::ApplyStarted)
            | (
                OperationPhase::ApplyStarted,
                OperationPhase::Applied
                    | OperationPhase::ApplyFailed
                    | OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::Applied,
                OperationPhase::Verified
                    | OperationPhase::VerifyFailed
                    | OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::Verified,
                OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::VerifyFailed,
                OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::ApplyFailed,
                OperationPhase::RolledBack
                    | OperationPhase::RollbackFailed
                    | OperationPhase::ForwardRecovered
                    | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::Prepared,
                OperationPhase::ForwardRecovered | OperationPhase::ForwardRecoveryFailed
            )
            | (
                OperationPhase::ForwardRecoveryFailed,
                OperationPhase::ForwardRecovered | OperationPhase::ForwardRecoveryFailed
            )
    )
}

fn legal_transition(from: ReceiptState, to: ReceiptState) -> bool {
    matches!(
        (from, to),
        (
            ReceiptState::Prepared,
            ReceiptState::Applying | ReceiptState::Canceled
        ) | (
            ReceiptState::Applying,
            ReceiptState::Verifying
                | ReceiptState::RecoveryRequired
                | ReceiptState::ForwardRecoveryRequired
        ) | (
            ReceiptState::Verifying,
            ReceiptState::Succeeded
                | ReceiptState::ApplyingForward
                | ReceiptState::RecoveryRequired
                | ReceiptState::ForwardRecoveryRequired
        ) | (
            ReceiptState::ApplyingForward,
            ReceiptState::Succeeded
                | ReceiptState::RecoveryRequired
                | ReceiptState::ForwardRecoveryRequired
        ) | (ReceiptState::RecoveryRequired, ReceiptState::RollingBack)
            | (
                ReceiptState::ForwardRecoveryRequired,
                ReceiptState::ConvergingForward
            )
            | (
                ReceiptState::ConvergingForward,
                ReceiptState::ForwardRecovered | ReceiptState::ForwardRecoveryFailed
            )
            | (
                ReceiptState::ForwardRecoveryFailed,
                ReceiptState::ConvergingForward
            )
            | (ReceiptState::Succeeded, ReceiptState::RollingBack)
            | (
                ReceiptState::RollingBack,
                ReceiptState::RolledBack | ReceiptState::RollbackFailed
            )
    )
}

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error("receipt schema/contract version is unsupported: {0}")]
    UnsupportedSchemaVersion(u32),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("illegal receipt transition from {from:?} to {to:?}")]
    IllegalTransition {
        from: ReceiptState,
        to: ReceiptState,
    },
    #[error("receipt transition hash chain is invalid")]
    InvalidHashChain,
    #[error("operation progress transition is invalid")]
    InvalidOperationProgress,
    #[error("package receipt authorization or evidence is invalid")]
    InvalidPackageAuthorization,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("receipt not found for run {0}")]
    NotFound(StableId),
    #[error("receipt snapshot filename is invalid")]
    InvalidSnapshotName,
    #[error("receipt snapshots contain a sequence gap")]
    SnapshotGap,
    #[error("receipt snapshots contain conflicting history")]
    ConflictingHistory,
    #[error("receipt store contains an invalid run directory")]
    InvalidRunDirectory,
    #[error(
        "stale receipt write at sequence {attempted_sequence}; persisted sequence is {persisted_sequence}"
    )]
    StaleWrite {
        persisted_sequence: u64,
        attempted_sequence: u64,
    },
}

#[cfg(test)]
mod receipt_chain_tests {
    use super::*;

    fn digest(character: char) -> Sha256Digest {
        Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
    }

    fn journal() -> ReceiptJournal {
        ReceiptJournal::new(
            StableId::parse("tampered-run").unwrap(),
            digest('a'),
            StableId::parse("target").unwrap(),
            digest('b'),
            digest('c'),
            digest('d'),
            PlanBindings {
                target_identity_digest: digest('e'),
                composed_loadout_digest: digest('f'),
                provider_inputs_digest: digest('1'),
                ownership_map_digest: digest('2'),
                artifact_set_digest: digest('3'),
                package_resolution_authority_digest: None,
            },
        )
        .unwrap()
    }

    fn replace_states(journal: &mut ReceiptJournal, states: &[ReceiptState]) {
        let progress_digest = digest_domain_json(
            "commonkit.operation-progress.v1",
            &journal.receipt.operation_progress,
        )
        .unwrap();
        let mut transitions = Vec::new();
        let mut previous = None;
        for (sequence, state) in states.iter().copied().enumerate() {
            let transition =
                make_transition(sequence as u64, state, previous, progress_digest.clone()).unwrap();
            previous = Some(transition.entry_digest.clone());
            transitions.push(transition);
        }
        journal.receipt.receipt_id = previous.unwrap();
        journal.receipt.transitions = transitions;
    }

    #[test]
    fn hash_valid_receipts_reject_invalid_state_history_and_top_level_state() {
        let mut invalid_first = journal();
        replace_states(&mut invalid_first, &[ReceiptState::Applying]);
        invalid_first.receipt.state = ReceiptState::Applying;
        assert!(matches!(
            invalid_first.verify_chain(),
            Err(ReceiptError::InvalidHashChain)
        ));

        let mut illegal_adjacent = journal();
        replace_states(
            &mut illegal_adjacent,
            &[ReceiptState::Prepared, ReceiptState::Succeeded],
        );
        illegal_adjacent.receipt.state = ReceiptState::Succeeded;
        assert!(matches!(
            illegal_adjacent.verify_chain(),
            Err(ReceiptError::InvalidHashChain)
        ));

        let mut mismatched_top = journal();
        replace_states(
            &mut mismatched_top,
            &[ReceiptState::Prepared, ReceiptState::Applying],
        );
        mismatched_top.receipt.state = ReceiptState::Prepared;
        assert!(matches!(
            mismatched_top.verify_chain(),
            Err(ReceiptError::InvalidHashChain)
        ));
    }
}

#[cfg(all(test, unix))]
mod plan_file_tests {
    use super::*;

    #[test]
    fn opened_plan_descriptor_is_not_affected_by_path_replacement() {
        let root = std::env::temp_dir().join(format!(
            "commonkit-plan-descriptor-{}-{}",
            std::process::id(),
            PLAN_TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("temporary directory");
        let path = root.join("plan.json");
        fs::write(&path, b"original").expect("original plan");
        let (mut descriptor, _) = open_plan_file_from_path(&path).expect("open plan");

        fs::rename(&path, root.join("old-plan.json")).expect("replace original path");
        fs::write(&path, b"replacement").expect("replacement plan");

        let mut bytes = Vec::new();
        descriptor
            .read_to_end(&mut bytes)
            .expect("read opened descriptor");
        assert_eq!(bytes, b"original");
        fs::remove_dir_all(root).expect("cleanup");
    }
}

#[cfg(all(test, windows))]
mod plan_file_windows_tests {
    use super::*;

    #[test]
    fn plan_file_reparse_attribute_guard_compiles_and_accepts_a_normal_file() {
        let path = std::env::temp_dir().join(format!(
            "commonkit-plan-file-{}-{}.json",
            std::process::id(),
            PLAN_TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, b"plan").expect("temporary plan");
        let metadata = fs::symlink_metadata(&path).expect("plan metadata");
        assert!(!metadata_is_reparse_point(&metadata));
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn windows_directory_identity_uses_the_stable_handle_api() {
        let path = std::env::temp_dir().join(format!(
            "commonkit-plan-root-{}-{}",
            std::process::id(),
            PLAN_TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("temporary root");
        let (_directory, handle) = open_plan_root(&path).expect("open root");
        assert!(same_directory(&handle, &handle).expect("identity"));
        handle.sync_all().expect("directory flush contract");
        fs::remove_dir(path).expect("cleanup");
    }
}
