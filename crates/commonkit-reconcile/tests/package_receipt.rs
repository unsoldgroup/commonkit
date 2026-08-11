use std::collections::BTreeSet;

use commonkit_contracts::{
    OperationKind, PackageExitClassification, PackageManager, PackageNoPreimageReason,
    PackageReceiptAuthorization, PackageReceiptEvidence, PlanBindings, RecoveryCapability,
    ResourceRef, Risk, Sha256Digest, StableId,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::ReceiptJournal;

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn plan() -> commonkit_contracts::Plan {
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

fn authorization(operation_id: Sha256Digest) -> PackageReceiptAuthorization {
    PackageReceiptAuthorization {
        consent_digest: digest('7'),
        confirmation_id: StableId::parse("confirm-packages").unwrap(),
        operation_set_digest: digest('8'),
        evidence: vec![PackageReceiptEvidence {
            operation_id,
            resolution_digest: digest('b'),
            target_authority_digest: digest('1'),
            manager: PackageManager::Apt,
            manager_authority_digest: digest('9'),
            source_id: StableId::parse("ubuntu-main").unwrap(),
            source_authority_digest: digest('0'),
            before_installed_versions: BTreeSet::from(["ripgrep:amd64=14.1.0".into()]),
            no_preimage_reason: PackageNoPreimageReason::AdditiveForwardOnly,
            exit_classification: PackageExitClassification::NotRun,
            final_digest: None,
        }],
    }
}

#[test]
fn package_authorization_and_evidence_are_bound_into_v3_receipt_chain() {
    let plan = plan();
    let operation_id = plan.operations[0].id.clone();
    let mut journal = ReceiptJournal::for_package_plan(
        StableId::parse("package-run").unwrap(),
        &plan,
        authorization(operation_id.clone()),
    )
    .unwrap();
    let initial_id = journal.receipt().receipt_id.clone();

    journal
        .record_package_exit(
            &operation_id,
            PackageExitClassification::Succeeded,
            Some(digest('f')),
        )
        .unwrap();

    assert_eq!(journal.receipt().schema_version.0, 3);
    assert_ne!(journal.receipt().receipt_id, initial_id);
    assert_eq!(
        journal
            .receipt()
            .package_authorization
            .as_ref()
            .unwrap()
            .evidence[0]
            .exit_classification,
        PackageExitClassification::Succeeded
    );
    journal.verify_chain().unwrap();
}

#[test]
fn legacy_receipts_keep_package_authorization_absent() {
    let plan = plan();
    let journal = ReceiptJournal::for_plan(StableId::parse("legacy-run").unwrap(), &plan).unwrap();

    assert_eq!(journal.receipt().schema_version.0, 2);
    assert!(journal.receipt().package_authorization.is_none());
}
