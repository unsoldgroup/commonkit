use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{OperationKind, PlanBindings, ResourceRef, Risk, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{PlanStore, PlanStoreError};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn plan() -> commonkit_contracts::Plan {
    build_plan(PlanDraft {
        target_id: StableId::parse("local-target").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: PlanBindings {
            target_identity_digest: digest('3'),
            composed_loadout_digest: digest('d'),
            provider_inputs_digest: digest('e'),
            ownership_map_digest: digest('f'),
            artifact_set_digest: digest('1'),
        },
        operations: Vec::new(),
    })
    .expect("plan")
}

#[test]
fn persists_loads_and_rejects_a_tampered_content_addressed_plan() {
    let root = temporary_directory("plan-store");
    let store = PlanStore::open(&root).expect("store");
    let plan = plan();
    store.persist(&plan).expect("persist");
    assert_eq!(store.load(&plan.id).expect("load"), plan);

    let path = fs::read_dir(&root)
        .expect("plans")
        .next()
        .expect("plan file")
        .expect("entry")
        .path();
    let mut bytes = fs::read(&path).expect("plan bytes");
    let index = bytes.len() / 2;
    bytes[index] ^= 1;
    fs::write(path, bytes).expect("tamper");
    assert!(matches!(
        store.load(&plan.id),
        Err(PlanStoreError::InvalidPlan | PlanStoreError::Serialization(_))
    ));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn reloads_a_pre_provenance_v1_operation_without_changing_its_identity() {
    let root = temporary_directory("legacy-plan-store");
    let store = PlanStore::open(&root).expect("store");
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("filesystem").expect("adapter"),
        kind: OperationKind::Update,
        resource: ResourceRef {
            resource_type: StableId::parse("file").expect("type"),
            resource_id: StableId::parse("config").expect("resource"),
            managed_path: Some("config.txt".into()),
        },
        risk: Risk::Low,
        requires_confirmation: true,
        recovery_capability: commonkit_contracts::RecoveryCapability::ExactRollback,
        depends_on: Vec::new(),
        before_digest: Some(digest('4')),
        after_digest: Some(digest('5')),
        payload_digest: digest('6'),
        provenance: None,
        summary: "legacy durable operation".into(),
    })
    .expect("operation");
    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("local-target").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: PlanBindings {
            target_identity_digest: digest('3'),
            composed_loadout_digest: digest('d'),
            provider_inputs_digest: digest('e'),
            ownership_map_digest: digest('f'),
            artifact_set_digest: digest('1'),
        },
        operations: vec![operation],
    })
    .expect("plan");

    let serialized = serde_json::to_string(&plan).expect("serialize");
    assert!(
        !serialized.contains("provenance"),
        "the legacy v1 representation must remain byte-shape compatible"
    );
    assert!(
        !serialized.contains("recoveryCapability"),
        "the default exact rollback capability must preserve v1 plan bytes"
    );
    store.persist(&plan).expect("persist");
    assert_eq!(store.load(&plan.id).expect("reload"), plan);

    fs::remove_dir_all(root).expect("cleanup");
}
