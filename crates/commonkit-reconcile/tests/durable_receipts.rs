use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{PlanBindings, ReceiptState, Sha256Digest, StableId};
use commonkit_reconcile::{ReceiptError, ReceiptJournal, ReceiptStore};

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

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn journal(run_id: &str) -> ReceiptJournal {
    ReceiptJournal::new(
        StableId::parse(run_id).expect("run"),
        digest('a'),
        StableId::parse("laptop").expect("target"),
        digest('b'),
        digest('c'),
        digest('d'),
        bindings(),
    )
    .expect("journal")
}

#[test]
fn persists_each_transition_and_reloads_a_verified_chain() {
    let directory = temporary_directory("durable-reload");
    let store = ReceiptStore::open(&directory).expect("store");
    let mut receipt = journal("run-durable");

    store.persist(&receipt).expect("prepared persisted");
    receipt
        .transition(ReceiptState::Applying)
        .expect("applying");
    store.persist(&receipt).expect("applying persisted");

    let loaded = store
        .load(receipt.receipt().run_id.clone())
        .expect("loaded");
    assert_eq!(loaded.receipt(), receipt.receipt());
    loaded.verify_chain().expect("verified after reload");
    assert_eq!(
        fs::read_dir(directory.join("run-durable"))
            .expect("snapshots")
            .count(),
        2
    );

    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn rejects_a_tampered_receipt_on_load() {
    let directory = temporary_directory("durable-tamper");
    let store = ReceiptStore::open(&directory).expect("store");
    let receipt = journal("run-tampered");
    store.persist(&receipt).expect("persisted");
    let path = fs::read_dir(directory.join("run-tampered"))
        .expect("snapshots")
        .next()
        .expect("snapshot")
        .expect("entry")
        .path();
    let content = fs::read_to_string(&path).expect("read");
    fs::write(&path, content.replace("prepared", "succeeded")).expect("tamper");

    assert!(matches!(
        store.load(StableId::parse("run-tampered").expect("run")),
        Err(ReceiptError::InvalidHashChain)
    ));

    fs::remove_dir_all(directory).expect("cleanup");
}

#[test]
fn refuses_to_overwrite_a_newer_persisted_transition() {
    let directory = temporary_directory("durable-stale");
    let store = ReceiptStore::open(&directory).expect("store");
    let stale = journal("run-stale");
    store.persist(&stale).expect("prepared persisted");
    let mut newer = journal("run-stale");
    newer.transition(ReceiptState::Applying).expect("applying");
    store.persist(&newer).expect("applying persisted");

    assert!(matches!(
        store.persist(&stale),
        Err(ReceiptError::StaleWrite { .. })
    ));

    fs::remove_dir_all(directory).expect("cleanup");
}
