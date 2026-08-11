use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{
    Operation, OperationKind, PackageConsent, PackageExitClassification, PackageManager,
    PackageNoPreimageReason, PackageOperationConsentBinding, PackageReceiptEvidence, PlanBindings,
    RecoveryCapability, ResourceRef, Risk, Sha256Digest, StableId, package_operation_set_digest,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure, ReceiptStore, ReconcileError, Reconciler};

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
        Ok(())
    }
    fn rollback(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        panic!("package mutation cannot exact rollback")
    }
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
