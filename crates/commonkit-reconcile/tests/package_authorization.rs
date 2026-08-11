use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{
    Operation, OperationKind, OperationPhase, PackageConsent, PackageExitClassification,
    PackageManager, PackageNoPreimageReason, PackageOperationConsentBinding,
    PackageReceiptAuthorization, PackageReceiptEvidence, PlanBindings, ReceiptState,
    RecoveryCapability, ResourceRef, Risk, Sha256Digest, StableId, package_operation_set_digest,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{
    Adapter, AdapterFailure, ReceiptStore, ReconcileError, ReconcileOutcome, Reconciler,
};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn package_plan() -> commonkit_contracts::Plan {
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("packages").unwrap(),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("package").unwrap(),
            resource_id: StableId::parse("ripgrep").unwrap(),
            managed_path: None,
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        recovery_capability: RecoveryCapability::ConvergeForwardOnly,
        depends_on: vec![],
        before_digest: None,
        after_digest: Some(digest('a')),
        payload_digest: digest('b'),
        provenance: None,
        summary: "install ripgrep".into(),
    })
    .unwrap();
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").unwrap(),
        desired_digest: digest('c'),
        observed_digest: digest('d'),
        policy_digest: digest('e'),
        bindings: PlanBindings {
            target_identity_digest: digest('1'),
            composed_loadout_digest: digest('2'),
            provider_inputs_digest: digest('3'),
            ownership_map_digest: digest('4'),
            artifact_set_digest: digest('5'),
            package_resolution_authority_digest: Some(digest('6')),
        },
        operations: vec![operation],
    })
    .unwrap()
}

struct PackageAdapterStub {
    id: StableId,
    mutation_count: usize,
    forbid_observation: bool,
    verify_failure: bool,
}

impl Adapter for PackageAdapterStub {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn supports_recovery(&self, capability: RecoveryCapability) -> bool {
        capability == RecoveryCapability::ConvergeForwardOnly
    }

    fn supports_offline_recovery(&self, _operation: &Operation) -> bool {
        true
    }

    fn observe_recovery(
        &mut self,
        _operation: &Operation,
    ) -> Result<commonkit_reconcile::RecoveryObservation, AdapterFailure> {
        assert!(
            !self.forbid_observation,
            "recovery must use durable evidence only"
        );
        Ok(commonkit_reconcile::RecoveryObservation::After)
    }

    fn package_authorization_binding(
        &self,
        operation: &Operation,
    ) -> Result<Option<(PackageOperationConsentBinding, PackageReceiptEvidence)>, AdapterFailure>
    {
        Ok(Some((
            PackageOperationConsentBinding {
                operation_id: operation.id.clone(),
                resolution_digest: operation.payload_digest.clone(),
            },
            PackageReceiptEvidence {
                operation_id: operation.id.clone(),
                resolution_digest: operation.payload_digest.clone(),
                target_authority_digest: digest('1'),
                manager: PackageManager::Apt,
                manager_authority_digest: digest('9'),
                source_id: StableId::parse("ubuntu-main").unwrap(),
                source_authority_digest: digest('0'),
                before_installed_versions: BTreeSet::new(),
                no_preimage_reason: PackageNoPreimageReason::AdditiveForwardOnly,
                exit_classification: PackageExitClassification::NotRun,
                final_digest: None,
            },
        )))
    }

    fn package_final_digest(
        &mut self,
        _operation: &Operation,
    ) -> Result<Option<Sha256Digest>, AdapterFailure> {
        Ok(Some(digest('f')))
    }

    fn prepare(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn apply(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        self.mutation_count += 1;
        Ok(())
    }
    fn verify(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        if self.verify_failure {
            Err(AdapterFailure::new(
                "package_verify_failed",
                "backend output must not enter receipt evidence",
            ))
        } else {
            Ok(())
        }
    }
    fn rollback(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        panic!("package mutation cannot exact rollback")
    }
}

fn package_authorization(operation: &Operation) -> PackageReceiptAuthorization {
    PackageReceiptAuthorization {
        consent_digest: digest('1'),
        confirmation_id: StableId::parse("confirm-packages").unwrap(),
        operation_set_digest: digest('2'),
        evidence: vec![PackageReceiptEvidence {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
            target_authority_digest: digest('3'),
            manager: PackageManager::Apt,
            manager_authority_digest: digest('4'),
            source_id: StableId::parse("ubuntu-main").unwrap(),
            source_authority_digest: digest('5'),
            before_installed_versions: BTreeSet::new(),
            no_preimage_reason: PackageNoPreimageReason::AdditiveForwardOnly,
            exit_classification: PackageExitClassification::NotRun,
            final_digest: None,
        }],
    }
}

#[test]
fn interrupted_package_recovery_accepts_v2_plan_bound_to_v3_receipt() {
    let root = temporary_directory("package-recovery-v2-v3");
    let store = ReceiptStore::open(&root).unwrap();
    let plan = package_plan();
    let run_id = StableId::parse("package-interrupted").unwrap();
    let operation = &plan.operations[0];
    let mut authorization = package_authorization(operation);
    authorization.operation_set_digest = package_operation_set_digest(
        &plan,
        &[PackageOperationConsentBinding {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
        }],
    )
    .unwrap();
    let mut journal =
        commonkit_reconcile::ReceiptJournal::for_package_plan(run_id.clone(), &plan, authorization)
            .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
        .unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)
        .unwrap();
    store.persist(&journal).unwrap();

    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(PackageAdapterStub {
        id: StableId::parse("packages").unwrap(),
        mutation_count: 0,
        forbid_observation: false,
        verify_failure: false,
    })];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &plan, &mut adapters)
        .unwrap();

    assert_eq!(outcome, ReconcileOutcome::ForwardRecovered);
    assert_eq!(
        store.load(run_id).unwrap().receipt().state,
        ReceiptState::ForwardRecovered
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_recovery_repairs_forward_evidence_gap_without_target_or_provider_work() {
    let root = temporary_directory("package-recovery-evidence-gap");
    let store = ReceiptStore::open(&root).unwrap();
    let plan = package_plan();
    let run_id = StableId::parse("package-evidence-gap").unwrap();
    let operation = &plan.operations[0];
    let mut authorization = package_authorization(operation);
    authorization.operation_set_digest = package_operation_set_digest(
        &plan,
        &[PackageOperationConsentBinding {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
        }],
    )
    .unwrap();
    let mut journal =
        commonkit_reconcile::ReceiptJournal::for_package_plan(run_id.clone(), &plan, authorization)
            .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
        .unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Verifying).unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::ApplyingForward).unwrap();
    store.persist(&journal).unwrap();
    for phase in [
        OperationPhase::ApplyStarted,
        OperationPhase::Applied,
        OperationPhase::Verified,
    ] {
        journal
            .record_operation(operation.id.clone(), phase, None)
            .unwrap();
        store.persist(&journal).unwrap();
    }
    journal
        .transition(ReceiptState::ForwardRecoveryRequired)
        .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::ForwardRecovered, None)
        .unwrap();
    store.persist(&journal).unwrap();

    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(PackageAdapterStub {
        id: StableId::parse("packages").unwrap(),
        mutation_count: 0,
        forbid_observation: true,
        verify_failure: false,
    })];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &plan, &mut adapters)
        .unwrap();

    assert_eq!(outcome, ReconcileOutcome::ForwardRecovered);
    let receipt = store.load(run_id).unwrap();
    let evidence = &receipt
        .receipt()
        .package_authorization
        .as_ref()
        .unwrap()
        .evidence[0];
    assert_eq!(
        evidence.exit_classification,
        PackageExitClassification::Succeeded
    );
    assert_eq!(evidence.final_digest, Some(digest('f')));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_recovery_repairs_forward_recovered_failed_evidence_gap_without_target_or_provider_work()
{
    let root = temporary_directory("package-recovery-failed-evidence-gap");
    let store = ReceiptStore::open(&root).unwrap();
    let plan = package_plan();
    let run_id = StableId::parse("package-failed-evidence-gap").unwrap();
    let operation = &plan.operations[0];
    let mut authorization = package_authorization(operation);
    authorization.operation_set_digest = package_operation_set_digest(
        &plan,
        &[PackageOperationConsentBinding {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
        }],
    )
    .unwrap();
    let mut journal =
        commonkit_reconcile::ReceiptJournal::for_package_plan(run_id.clone(), &plan, authorization)
            .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
        .unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Verifying).unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::ApplyingForward).unwrap();
    store.persist(&journal).unwrap();
    for phase in [
        OperationPhase::ApplyStarted,
        OperationPhase::Applied,
        OperationPhase::Verified,
    ] {
        journal
            .record_operation(operation.id.clone(), phase, None)
            .unwrap();
        store.persist(&journal).unwrap();
    }
    journal
        .transition(ReceiptState::ForwardRecoveryRequired)
        .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_package_exit(
            &operation.id,
            PackageExitClassification::Failed {
                code: StableId::parse("legacy_recovery_failure").unwrap(),
            },
            None,
        )
        .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::ForwardRecovered, None)
        .unwrap();
    store.persist(&journal).unwrap();

    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(PackageAdapterStub {
        id: StableId::parse("packages").unwrap(),
        mutation_count: 0,
        forbid_observation: true,
        verify_failure: false,
    })];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &plan, &mut adapters)
        .unwrap();

    assert_eq!(outcome, ReconcileOutcome::ForwardRecovered);
    let receipt = store.load(run_id).unwrap();
    let evidence = &receipt
        .receipt()
        .package_authorization
        .as_ref()
        .unwrap()
        .evidence[0];
    assert_eq!(
        evidence.exit_classification,
        PackageExitClassification::Succeeded
    );
    assert_eq!(evidence.final_digest, Some(digest('f')));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_recovery_repairs_failed_evidence_after_forward_failure_checkpoint() {
    let root = temporary_directory("package-recovery-failed-checkpoint");
    let store = ReceiptStore::open(&root).unwrap();
    let plan = package_plan();
    let run_id = StableId::parse("package-failed-checkpoint").unwrap();
    let operation = &plan.operations[0];
    let mut authorization = package_authorization(operation);
    authorization.operation_set_digest = package_operation_set_digest(
        &plan,
        &[PackageOperationConsentBinding {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
        }],
    )
    .unwrap();
    let mut journal =
        commonkit_reconcile::ReceiptJournal::for_package_plan(run_id.clone(), &plan, authorization)
            .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
        .unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::Verifying).unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::ApplyingForward).unwrap();
    store.persist(&journal).unwrap();
    for phase in [
        OperationPhase::ApplyStarted,
        OperationPhase::Applied,
        OperationPhase::Verified,
        OperationPhase::ForwardRecoveryFailed,
    ] {
        journal
            .record_operation(
                operation.id.clone(),
                phase,
                (phase == OperationPhase::ForwardRecoveryFailed)
                    .then(|| StableId::parse("legacy_recovery_failure").unwrap()),
            )
            .unwrap();
        store.persist(&journal).unwrap();
    }
    journal
        .transition(ReceiptState::ForwardRecoveryRequired)
        .unwrap();
    store.persist(&journal).unwrap();
    journal.transition(ReceiptState::ConvergingForward).unwrap();
    store.persist(&journal).unwrap();
    journal
        .transition(ReceiptState::ForwardRecoveryFailed)
        .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_package_exit(
            &operation.id,
            PackageExitClassification::Failed {
                code: StableId::parse("legacy_recovery_failure").unwrap(),
            },
            None,
        )
        .unwrap();
    store.persist(&journal).unwrap();
    journal
        .record_operation(operation.id.clone(), OperationPhase::ForwardRecovered, None)
        .unwrap();
    store.persist(&journal).unwrap();

    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(PackageAdapterStub {
        id: StableId::parse("packages").unwrap(),
        mutation_count: 0,
        forbid_observation: true,
        verify_failure: false,
    })];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &plan, &mut adapters)
        .unwrap();

    assert_eq!(outcome, ReconcileOutcome::ForwardRecovered);
    let receipt = store.load(run_id).unwrap();
    let evidence = &receipt
        .receipt()
        .package_authorization
        .as_ref()
        .unwrap()
        .evidence[0];
    assert_eq!(
        evidence.exit_classification,
        PackageExitClassification::Succeeded
    );
    assert_eq!(evidence.final_digest, Some(digest('f')));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_plan_requires_exact_consent_before_receipt_or_mutation() {
    let root = temporary_directory("package-authorization");
    let store = ReceiptStore::open(&root).unwrap();
    let plan = package_plan();
    let binding = PackageOperationConsentBinding {
        operation_id: plan.operations[0].id.clone(),
        resolution_digest: plan.operations[0].payload_digest.clone(),
    };
    let wrong = PackageConsent {
        confirmation_id: StableId::parse("confirm-packages").unwrap(),
        operation_set_digest: digest('f'),
    };
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(PackageAdapterStub {
        id: StableId::parse("packages").unwrap(),
        mutation_count: 0,
        forbid_observation: false,
        verify_failure: false,
    })];
    let before = std::fs::read_dir(&root).unwrap().count();

    assert!(matches!(
        Reconciler::with_store(&store).execute(
            &plan,
            StableId::parse("missing-consent").unwrap(),
            &mut adapters,
        ),
        Err(ReconcileError::PackageConsentRequired)
    ));
    assert!(
        Reconciler::with_store(&store)
            .execute_with_package_consent(
                &plan,
                StableId::parse("wrong-consent").unwrap(),
                &wrong,
                &mut adapters,
            )
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);

    let consent = PackageConsent {
        confirmation_id: StableId::parse("confirm-packages").unwrap(),
        operation_set_digest: package_operation_set_digest(&plan, &[binding]).unwrap(),
    };
    Reconciler::with_store(&store)
        .execute_with_package_consent(
            &plan,
            StableId::parse("valid-consent").unwrap(),
            &consent,
            &mut adapters,
        )
        .unwrap();
    let receipt = store
        .load(StableId::parse("valid-consent").unwrap())
        .unwrap();
    let authorization = receipt.receipt().package_authorization.as_ref().unwrap();
    assert_eq!(
        authorization.evidence[0].exit_classification,
        PackageExitClassification::Succeeded
    );
    assert_eq!(authorization.evidence[0].final_digest, Some(digest('f')));
}

#[test]
fn package_verify_failure_records_only_sanitized_failed_evidence() {
    let root = temporary_directory("package-verify-failure");
    let store = ReceiptStore::open(&root).unwrap();
    let plan = package_plan();
    let operation = &plan.operations[0];
    let mut authorization = package_authorization(operation);
    authorization.operation_set_digest = package_operation_set_digest(
        &plan,
        &[PackageOperationConsentBinding {
            operation_id: operation.id.clone(),
            resolution_digest: operation.payload_digest.clone(),
        }],
    )
    .unwrap();
    let consent = PackageConsent {
        confirmation_id: authorization.confirmation_id.clone(),
        operation_set_digest: authorization.operation_set_digest.clone(),
    };
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(PackageAdapterStub {
        id: StableId::parse("packages").unwrap(),
        mutation_count: 0,
        forbid_observation: false,
        verify_failure: true,
    })];

    let outcome = Reconciler::with_store(&store)
        .execute_with_package_consent(
            &plan,
            StableId::parse("verify-failure").unwrap(),
            &consent,
            &mut adapters,
        )
        .unwrap();

    assert_eq!(outcome, ReconcileOutcome::ForwardRecoveryRequired);
    let receipt = store
        .load(StableId::parse("verify-failure").unwrap())
        .unwrap();
    let evidence = &receipt
        .receipt()
        .package_authorization
        .as_ref()
        .unwrap()
        .evidence[0];
    assert_eq!(
        evidence.exit_classification,
        PackageExitClassification::Failed {
            code: StableId::parse("package_verify_failed").unwrap(),
        }
    );
    assert_eq!(evidence.final_digest, None);
    std::fs::remove_dir_all(root).unwrap();
}
