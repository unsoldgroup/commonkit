use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{PlanBindings, Sha256Digest, StableId};
use commonkit_core::{PlanDraft, build_plan};
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
