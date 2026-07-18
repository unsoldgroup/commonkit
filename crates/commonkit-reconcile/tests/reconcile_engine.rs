use std::sync::{Arc, Mutex};

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{
    Operation, OperationKind, OperationPhase, ReceiptState, ResourceRef, Risk, Sha256Digest,
    StableId,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{
    Adapter, AdapterFailure, ReceiptJournal, ReceiptStore, ReconcileOutcome, Reconciler,
};

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

fn operation(adapter: &str, resource: &str) -> Operation {
    finalize_operation(OperationDraft {
        adapter_id: StableId::parse(adapter).expect("adapter"),
        kind: OperationKind::Update,
        resource: ResourceRef {
            resource_type: StableId::parse("file").expect("type"),
            resource_id: StableId::parse(resource).expect("resource"),
            managed_path: None,
        },
        risk: Risk::Low,
        requires_confirmation: false,
        depends_on: vec![],
        before_digest: Some(digest('1')),
        after_digest: Some(digest('2')),
        summary: resource.into(),
    })
    .expect("operation")
}

struct RecordingAdapter {
    id: StableId,
    events: Arc<Mutex<Vec<String>>>,
    fail_apply_for: Option<String>,
    fail_verify_for: Option<String>,
    fail_rollback_for: Option<String>,
}

impl Adapter for RecordingAdapter {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("prepare", operation);
        Ok(())
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("apply", operation);
        self.fail_if(&self.fail_apply_for, operation)
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("verify", operation);
        self.fail_if(&self.fail_verify_for, operation)
    }

    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("rollback", operation);
        self.fail_if(&self.fail_rollback_for, operation)
    }
}

impl RecordingAdapter {
    fn record(&self, phase: &str, operation: &Operation) {
        self.events
            .lock()
            .expect("events")
            .push(format!("{phase}:{}", operation.resource.resource_id));
    }

    fn fail_if(
        &self,
        configured: &Option<String>,
        operation: &Operation,
    ) -> Result<(), AdapterFailure> {
        if configured.as_deref() == Some(operation.resource.resource_id.as_str()) {
            Err(AdapterFailure::new("injected_failure", "injected failure"))
        } else {
            Ok(())
        }
    }
}

fn adapter(events: Arc<Mutex<Vec<String>>>) -> RecordingAdapter {
    RecordingAdapter {
        id: StableId::parse("files").expect("adapter"),
        events,
        fail_apply_for: None,
        fail_verify_for: None,
        fail_rollback_for: None,
    }
}

fn plan() -> commonkit_contracts::Plan {
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        operations: vec![operation("files", "alpha"), operation("files", "beta")],
    })
    .expect("plan")
}

#[test]
fn prepares_every_operation_before_mutation_then_applies_and_verifies_in_plan_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(adapter(events.clone()))];
    let outcome = Reconciler::new()
        .execute(
            &plan(),
            StableId::parse("run-success").expect("run"),
            &mut adapters,
        )
        .expect("execute");

    assert_eq!(outcome, ReconcileOutcome::Succeeded);
    assert_eq!(
        *events.lock().expect("events"),
        [
            "prepare:alpha",
            "prepare:beta",
            "apply:alpha",
            "apply:beta",
            "verify:alpha",
            "verify:beta"
        ]
    );
}

#[test]
fn rolls_back_only_applied_operations_in_reverse_order_after_apply_failure() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut failing = adapter(events.clone());
    failing.fail_apply_for = Some("beta".into());
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(failing)];
    let result = Reconciler::new()
        .execute(
            &plan(),
            StableId::parse("run-rollback").expect("run"),
            &mut adapters,
        )
        .expect("execute");

    assert_eq!(result, ReconcileOutcome::RolledBack);
    assert_eq!(
        *events.lock().expect("events"),
        [
            "prepare:alpha",
            "prepare:beta",
            "apply:alpha",
            "apply:beta",
            "rollback:alpha"
        ]
    );
}

#[test]
fn reports_recovery_required_when_rollback_fails() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut failing = adapter(events);
    failing.fail_verify_for = Some("beta".into());
    failing.fail_rollback_for = Some("alpha".into());
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(failing)];
    let result = Reconciler::new()
        .execute(
            &plan(),
            StableId::parse("run-recovery").expect("run"),
            &mut adapters,
        )
        .expect("execute");

    assert_eq!(result, ReconcileOutcome::RollbackFailed);
    assert_eq!(
        ReconcileOutcome::RollbackFailed.receipt_state(),
        ReceiptState::RollbackFailed
    );
}

#[test]
fn restart_recovery_rolls_back_durably_recorded_operations_in_reverse_order() {
    let directory = temporary_directory("restart-recovery");
    let store = ReceiptStore::open(&directory).expect("store");
    let plan = plan();
    let run_id = StableId::parse("run-crashed").expect("run");
    let mut journal = ReceiptJournal::new(
        run_id.clone(),
        plan.id.clone(),
        plan.target_id.clone(),
        plan.desired_digest.clone(),
        plan.observed_digest.clone(),
        plan.policy_digest.clone(),
    )
    .expect("journal");
    store.persist(&journal).expect("initial checkpoint");
    for operation in &plan.operations {
        journal
            .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
            .expect("prepared");
        store.persist(&journal).expect("prepared checkpoint");
    }
    journal
        .transition(ReceiptState::Applying)
        .expect("applying");
    store.persist(&journal).expect("applying checkpoint");
    for operation in &plan.operations {
        journal
            .record_operation(operation.id.clone(), OperationPhase::Applied, None)
            .expect("applied");
        store.persist(&journal).expect("applied checkpoint");
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(adapter(events.clone()))];
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &plan, &mut adapters)
        .expect("recover");

    assert_eq!(outcome, ReconcileOutcome::RolledBack);
    assert_eq!(
        *events.lock().expect("events"),
        ["rollback:beta", "rollback:alpha"]
    );
    assert_eq!(
        store.load(run_id).expect("receipt").receipt().state,
        ReceiptState::RolledBack
    );
    std::fs::remove_dir_all(directory).expect("cleanup");
}
