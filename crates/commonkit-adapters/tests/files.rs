use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{FileAdapter, FileIntent, ManagedRelativePath};
use commonkit_contracts::{OperationPhase, PlanBindings, ReceiptState, Sha256Digest, StableId};
use commonkit_core::{PlanDraft, build_plan};
use commonkit_reconcile::{Adapter, ReceiptJournal, ReceiptStore, ReconcileOutcome, Reconciler};
use sha2::{Digest, Sha256};

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    let digest = Sha256::digest(bytes);
    Sha256Digest::parse(format!("sha256:{digest:x}")).expect("digest")
}

fn bindings() -> PlanBindings {
    PlanBindings {
        target_identity_digest: digest(b"target"),
        composed_loadout_digest: digest(b"loadout"),
        provider_inputs_digest: digest(b"inputs"),
        ownership_map_digest: digest(b"ownership"),
        artifact_set_digest: digest(b"artifacts"),
    }
}

fn intent(path: &str, content: &[u8], expected_before: Option<Sha256Digest>) -> FileIntent {
    FileIntent {
        id: StableId::parse("managed-config").expect("id"),
        path: ManagedRelativePath::parse(path).expect("path"),
        content: content.to_vec(),
        expected_before,
    }
}

#[test]
fn rejects_absolute_traversal_windows_and_git_paths() {
    for path in [
        "",
        "/etc/passwd",
        "../outside",
        "a/../outside",
        "C:\\secret",
        ".git/config",
    ] {
        assert!(ManagedRelativePath::parse(path).is_err(), "accepted {path}");
    }
    assert!(ManagedRelativePath::parse("agents/config.json").is_ok());
}

#[test]
fn creates_verifies_and_removes_a_managed_file_on_rollback() {
    let root = temporary_directory("file-create");
    let target = root.join("target");
    let state = root.join("state");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register(intent("nested/config.json", b"{\"enabled\":true}\n", None))
        .expect("operation");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    assert_eq!(
        fs::read(target.join("nested/config.json")).expect("file"),
        b"{\"enabled\":true}\n"
    );
    adapter.rollback(&operation).expect("rollback");
    assert!(!target.join("nested/config.json").exists());

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn restores_the_exact_preimage_and_rejects_changes_after_planning() {
    let root = temporary_directory("file-update");
    let target = root.join("target");
    let state = root.join("state");
    fs::create_dir_all(&target).expect("target");
    fs::write(target.join("config.txt"), b"before").expect("preimage");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register(intent("config.txt", b"after", Some(digest(b"before"))))
        .expect("operation");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    adapter.rollback(&operation).expect("rollback");
    assert_eq!(
        fs::read(target.join("config.txt")).expect("file"),
        b"before"
    );

    fs::write(target.join("config.txt"), b"changed externally").expect("drift");
    assert_eq!(
        adapter.prepare(&operation).expect_err("must reject").code,
        "preimage_changed"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn rejects_literal_secrets_in_portable_file_content() {
    let root = temporary_directory("file-secret");
    let mut adapter =
        FileAdapter::open(&root.join("target"), &root.join("state")).expect("adapter");
    assert!(
        adapter
            .register(intent("config.txt", b"API_KEY=literal-secret", None))
            .is_err()
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_fresh_process_recovers_an_applied_file_without_reconstructing_the_provider() {
    let root = temporary_directory("file-fresh-process-recovery");
    let target = root.join("target");
    let adapter_state = root.join("adapter-state");
    let receipt_state = root.join("receipts");
    fs::create_dir_all(&target).expect("target");
    fs::write(target.join("config.txt"), b"before").expect("preimage");

    let operation = {
        let mut adapter = FileAdapter::open(&target, &adapter_state).expect("first process");
        let operation = adapter
            .register(intent("config.txt", b"after", Some(digest(b"before"))))
            .expect("operation");
        adapter.prepare(&operation).expect("prepare");
        adapter.apply(&operation).expect("apply");
        operation
    };
    assert_eq!(
        fs::read(target.join("config.txt")).expect("applied"),
        b"after"
    );

    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("local-target").expect("target ID"),
        desired_digest: digest(b"desired"),
        observed_digest: digest(b"observed"),
        policy_digest: digest(b"policy"),
        bindings: bindings(),
        operations: vec![operation.clone()],
    })
    .expect("plan");
    let run_id = StableId::parse("run-file-crash").expect("run ID");
    let store = ReceiptStore::open(&receipt_state).expect("receipt store");
    let mut journal = ReceiptJournal::new(
        run_id.clone(),
        plan.id.clone(),
        plan.target_id.clone(),
        plan.desired_digest.clone(),
        plan.observed_digest.clone(),
        plan.policy_digest.clone(),
        plan.bindings.clone(),
    )
    .expect("journal");
    store.persist(&journal).expect("initial checkpoint");
    journal
        .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
        .expect("prepared checkpoint");
    store.persist(&journal).expect("persist prepared state");
    journal
        .transition(ReceiptState::Applying)
        .expect("applying checkpoint");
    store.persist(&journal).expect("persist applying state");
    journal
        .record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)
        .expect("apply-started checkpoint");
    store
        .persist(&journal)
        .expect("persist apply-started state");
    journal
        .record_operation(operation.id.clone(), OperationPhase::Applied, None)
        .expect("applied checkpoint");
    store.persist(&journal).expect("persist crash state");

    let fresh_adapter = FileAdapter::open(&target, &adapter_state).expect("fresh process");
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(fresh_adapter)];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &plan, &mut adapters)
        .expect("recover from durable plan and receipt");

    assert_eq!(outcome, ReconcileOutcome::RolledBack);
    assert_eq!(
        fs::read(target.join("config.txt")).expect("restored preimage"),
        b"before"
    );
    assert_eq!(
        store.load(run_id).expect("receipt").receipt().state,
        ReceiptState::RolledBack
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_fresh_process_recovers_when_the_target_changed_before_applied_was_recorded() {
    let root = temporary_directory("file-apply-checkpoint-crash");
    let target = root.join("target");
    let adapter_state = root.join("adapter-state");
    let receipt_state = root.join("receipts");
    fs::create_dir_all(&target).expect("target");
    fs::write(target.join("config.txt"), b"before").expect("preimage");

    let mut first_process = FileAdapter::open(&target, &adapter_state).expect("first process");
    let operation = first_process
        .register(intent("config.txt", b"after", Some(digest(b"before"))))
        .expect("operation");
    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("local-target").expect("target ID"),
        desired_digest: digest(b"desired"),
        observed_digest: digest(b"observed"),
        policy_digest: digest(b"policy"),
        bindings: bindings(),
        operations: vec![operation.clone()],
    })
    .expect("plan");
    let run_id = StableId::parse("run-apply-checkpoint-crash").expect("run ID");
    let store = ReceiptStore::open(&receipt_state).expect("receipt store");
    let mut journal = ReceiptJournal::new(
        run_id.clone(),
        plan.id.clone(),
        plan.target_id.clone(),
        plan.desired_digest.clone(),
        plan.observed_digest.clone(),
        plan.policy_digest.clone(),
        plan.bindings.clone(),
    )
    .expect("journal");
    store.persist(&journal).expect("initial checkpoint");
    first_process.prepare(&operation).expect("prepare");
    journal
        .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
        .expect("prepared checkpoint");
    store.persist(&journal).expect("persist prepared state");
    journal
        .transition(ReceiptState::Applying)
        .expect("applying checkpoint");
    store.persist(&journal).expect("persist applying state");
    journal
        .record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)
        .expect("apply-started checkpoint");
    store
        .persist(&journal)
        .expect("persist apply-started state");

    first_process.apply(&operation).expect("target mutation");
    drop(first_process);
    assert_eq!(
        fs::read(target.join("config.txt")).expect("applied"),
        b"after"
    );

    let fresh_adapter = FileAdapter::open(&target, &adapter_state).expect("fresh process");
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(fresh_adapter)];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id, &plan, &mut adapters)
        .expect("recover mutation whose completion checkpoint was interrupted");

    assert_eq!(outcome, ReconcileOutcome::RolledBack);
    assert_eq!(
        fs::read(target.join("config.txt")).expect("restored preimage"),
        b"before"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_fresh_adapter_rejects_a_mutation_descriptor_not_bound_to_the_operation() {
    let root = temporary_directory("file-descriptor-tamper");
    let target = root.join("target");
    let state = root.join("state");
    let operation = FileAdapter::open(&target, &state)
        .expect("adapter")
        .register(intent("config.txt", b"content", None))
        .expect("operation");
    let descriptor = fs::read_dir(state.join("operations"))
        .expect("operation records")
        .next()
        .expect("record")
        .expect("entry")
        .path();
    let original = fs::read_to_string(&descriptor).expect("descriptor");
    let tampered = original.replace(
        "\"sensitivity\":\"portable\"",
        "\"sensitivity\":\"local_sensitive\"",
    );
    assert_ne!(tampered, original, "fixture must alter the descriptor");
    fs::write(descriptor, tampered).expect("tamper descriptor");

    let mut fresh = FileAdapter::open(&target, &state).expect("fresh adapter");
    assert_eq!(
        fresh.apply(&operation).expect_err("tamper must fail").code,
        "operation_payload_mismatch"
    );
    assert!(!target.join("config.txt").exists());
    fs::remove_dir_all(root).expect("cleanup");
}
