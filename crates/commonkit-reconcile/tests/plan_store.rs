use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
            package_resolution_authority_digest: None,
        },
        operations: Vec::new(),
    })
    .expect("plan")
}

fn plan_for(target: &str, target_binding: char, policy: char) -> commonkit_contracts::Plan {
    build_plan(PlanDraft {
        target_id: StableId::parse(target).expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest(policy),
        bindings: PlanBindings {
            target_identity_digest: digest(target_binding),
            composed_loadout_digest: digest('d'),
            provider_inputs_digest: digest('e'),
            ownership_map_digest: digest('f'),
            artifact_set_digest: digest('1'),
            package_resolution_authority_digest: None,
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
            package_resolution_authority_digest: None,
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

#[test]
fn equal_mtime_plans_choose_the_stable_immutable_identity_tiebreaker() {
    let root = temporary_directory("plan-store-equal-mtime");
    let store = PlanStore::open(&root).expect("store");
    let first = plan_for("local-target", '3', 'c');
    let second = plan_for("local-target", '3', 'd');
    let (first, second) = if first.id < second.id {
        (first, second)
    } else {
        (second, first)
    };
    store.persist(&first).expect("persist first");
    store.persist(&second).expect("persist second");

    for entry in fs::read_dir(&root).expect("plans") {
        let path = entry.expect("entry").path();
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open plan");
        file.set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(60)))
            .expect("set equal mtime");
    }

    let selected = store
        .load_latest_for_target(&StableId::parse("local-target").expect("target"))
        .expect("load latest")
        .expect("matching plan");
    assert_eq!(selected.id, second.id);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn latest_matching_rejects_a_corrupt_known_plan_entry() {
    let root = temporary_directory("plan-store-corrupt-latest");
    let store = PlanStore::open(&root).expect("store");
    let plan = plan_for("local-target", '3', 'c');
    store.persist(&plan).expect("persist");
    let path = root.join(format!(
        "{}.json",
        plan.id.as_str().trim_start_matches("sha256:")
    ));
    fs::write(path, b"not a plan").expect("corrupt plan");

    assert!(matches!(
        store.load_latest_for_target(&StableId::parse("local-target").expect("target")),
        Err(PlanStoreError::InvalidPlan | PlanStoreError::Serialization(_))
    ));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn latest_matching_rejects_a_valid_plan_under_the_wrong_filename() {
    let root = temporary_directory("plan-store-wrong-filename");
    let store = PlanStore::open(&root).expect("store");
    let plan = plan_for("local-target", '3', 'c');
    store.persist(&plan).expect("persist");
    let path = root.join(format!(
        "{}.json",
        plan.id.as_str().trim_start_matches("sha256:")
    ));
    fs::rename(path, root.join("wrong-plan.json")).expect("rename plan");

    assert!(matches!(
        store.load_latest_for_target(&StableId::parse("local-target").expect("target")),
        Err(PlanStoreError::InvalidPlan)
    ));
    fs::remove_dir_all(root).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn latest_matching_rejects_a_symlink_plan_entry() {
    use std::os::unix::fs::symlink;

    let root = temporary_directory("plan-store-symlink-latest");
    let store = PlanStore::open(&root).expect("store");
    let plan = plan_for("local-target", '3', 'c');
    store.persist(&plan).expect("persist");
    let destination = root.join(format!(
        "{}.json",
        plan.id.as_str().trim_start_matches("sha256:")
    ));
    let target = root.join("outside.json");
    fs::rename(destination, &target).expect("move plan");
    symlink(
        &target,
        root.join(format!(
            "{}.json",
            plan.id.as_str().trim_start_matches("sha256:")
        )),
    )
    .expect("symlink plan");
    fs::remove_file(target).expect("remove symlink target");

    assert!(matches!(
        store.load_latest_for_target(&StableId::parse("local-target").expect("target")),
        Err(PlanStoreError::UnsafeEntry)
    ));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn latest_matching_rejects_an_unexpected_non_json_plan_artifact() {
    let root = temporary_directory("plan-store-unexpected-entry");
    let store = PlanStore::open(&root).expect("store");
    let plan = plan_for("local-target", '3', 'c');
    store.persist(&plan).expect("persist");
    let path = root.join(format!(
        "{}.json",
        plan.id.as_str().trim_start_matches("sha256:")
    ));
    fs::rename(path, root.join("plan.bak")).expect("rename plan artifact");

    assert!(matches!(
        store.load_latest_for_target(&StableId::parse("local-target").expect("target")),
        Err(PlanStoreError::InvalidPlan)
    ));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn latest_matching_allows_only_a_strict_inflight_plan_temp_entry() {
    let root = temporary_directory("plan-store-allowed-temp");
    let store = PlanStore::open(&root).expect("store");
    let plan = plan_for("local-target", '3', 'c');
    store.persist(&plan).expect("persist");
    fs::write(
        root.join(format!(
            ".plan-123-{}-0.tmp",
            plan.id.as_str().trim_start_matches("sha256:")
        )),
        b"inflight",
    )
    .expect("write inflight temp");
    fs::write(root.join(".plan-store.metadata"), b"metadata").expect("write metadata");

    assert_eq!(
        store
            .load_latest_for_target(&StableId::parse("local-target").expect("target"))
            .expect("load latest")
            .expect("matching plan")
            .id,
        plan.id
    );
    fs::remove_dir_all(root).expect("cleanup");
}
