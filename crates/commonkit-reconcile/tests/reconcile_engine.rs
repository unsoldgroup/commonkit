use std::sync::{Arc, Mutex};

use commonkit_contracts::{
    Operation, OperationKind, ReceiptState, ResourceRef, Risk, Sha256Digest, StableId,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure, ReconcileOutcome, Reconciler};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
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
