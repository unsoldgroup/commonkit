use commonkit_contracts::{
    OperationKind, PlanBindings, ReceiptState, RecoveryCapability, ResourceRef, Risk, Sha256Digest,
    StableId,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{ReceiptError, ReceiptJournal};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn bindings() -> PlanBindings {
    PlanBindings {
        target_identity_digest: digest('3'),
        composed_loadout_digest: digest('4'),
        provider_inputs_digest: digest('5'),
        ownership_map_digest: digest('6'),
        artifact_set_digest: digest('7'),
    }
}

fn forward_plan() -> commonkit_contracts::Plan {
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("packages").unwrap(),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("package").unwrap(),
            resource_id: StableId::parse("tool").unwrap(),
            managed_path: None,
        },
        risk: Risk::Low,
        requires_confirmation: true,
        recovery_capability: RecoveryCapability::ConvergeForwardOnly,
        depends_on: vec![],
        before_digest: None,
        after_digest: Some(digest('e')),
        payload_digest: digest('f'),
        provenance: None,
        summary: "install tool".into(),
    })
    .unwrap();
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").unwrap(),
        desired_digest: digest('b'),
        observed_digest: digest('c'),
        policy_digest: digest('d'),
        bindings: bindings(),
        operations: vec![operation],
    })
    .unwrap()
}

#[test]
fn records_a_hash_chained_success_path() {
    let mut journal = ReceiptJournal::new(
        StableId::parse("run-01").expect("run"),
        digest('a'),
        StableId::parse("laptop").expect("target"),
        digest('b'),
        digest('c'),
        digest('d'),
        bindings(),
    )
    .expect("journal");
    journal
        .transition(ReceiptState::Applying)
        .expect("applying");
    journal
        .transition(ReceiptState::Verifying)
        .expect("verifying");
    journal
        .transition(ReceiptState::Succeeded)
        .expect("succeeded");

    assert_eq!(journal.receipt().state, ReceiptState::Succeeded);
    assert_eq!(journal.receipt().transitions.len(), 4);
    journal.verify_chain().expect("valid chain");
}

#[test]
fn enforces_recovery_and_terminal_state_transitions() {
    let mut journal = ReceiptJournal::new(
        StableId::parse("run-02").expect("run"),
        digest('a'),
        StableId::parse("laptop").expect("target"),
        digest('b'),
        digest('c'),
        digest('d'),
        bindings(),
    )
    .expect("journal");
    assert!(matches!(
        journal.transition(ReceiptState::Succeeded),
        Err(ReceiptError::IllegalTransition { .. })
    ));
    journal
        .transition(ReceiptState::Applying)
        .expect("applying");
    journal
        .transition(ReceiptState::RecoveryRequired)
        .expect("recovery");
    journal
        .transition(ReceiptState::RollingBack)
        .expect("rolling back");
    journal
        .transition(ReceiptState::RolledBack)
        .expect("rolled back");
    assert!(matches!(
        journal.transition(ReceiptState::Applying),
        Err(ReceiptError::IllegalTransition { .. })
    ));
}

#[test]
fn records_one_way_forward_recovery_states() {
    let plan = forward_plan();
    let mut journal =
        ReceiptJournal::for_plan(StableId::parse("run-forward").unwrap(), &plan).unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    journal
        .transition(ReceiptState::ForwardRecoveryRequired)
        .unwrap();
    journal.transition(ReceiptState::ConvergingForward).unwrap();
    journal.transition(ReceiptState::ForwardRecovered).unwrap();
    assert!(matches!(
        journal.transition(ReceiptState::RollingBack),
        Err(ReceiptError::IllegalTransition { .. })
    ));
    journal.verify_chain().unwrap();
}

#[test]
fn legacy_v1_receipts_reject_forward_only_states() {
    let mut journal = ReceiptJournal::new(
        StableId::parse("legacy-run").unwrap(),
        digest('a'),
        StableId::parse("laptop").unwrap(),
        digest('b'),
        digest('c'),
        digest('d'),
        bindings(),
    )
    .unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    assert!(matches!(
        journal.transition(ReceiptState::ForwardRecoveryRequired),
        Err(ReceiptError::IllegalTransition { .. })
    ));
}
