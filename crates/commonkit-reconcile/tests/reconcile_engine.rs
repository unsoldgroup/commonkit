use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::{
    Operation, OperationKind, OperationPhase, PlanBindings, ReceiptState, RecoveryCapability,
    ResourceRef, Risk, Sha256Digest, StableId,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{
    Adapter, AdapterFailure, ReceiptJournal, ReceiptStore, ReconcileError, ReconcileOutcome,
    Reconciler, RecoveryObservation,
};

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
        package_resolution_authority_digest: None,
    }
}

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn operation(adapter: &str, resource: &str) -> Operation {
    operation_with_capability(adapter, resource, RecoveryCapability::ExactRollback)
}

fn operation_with_capability(
    adapter: &str,
    resource: &str,
    recovery_capability: RecoveryCapability,
) -> Operation {
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
        recovery_capability,
        depends_on: vec![],
        before_digest: Some(digest('1')),
        after_digest: Some(digest('2')),
        payload_digest: digest('3'),
        provenance: None,
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
    supports_forward: bool,
    unsupported_resource: Option<String>,
    observations: Arc<Mutex<BTreeMap<String, RecoveryObservation>>>,
}

struct ImplicitCapabilityAdapter {
    id: StableId,
}

impl Adapter for ImplicitCapabilityAdapter {
    fn id(&self) -> &StableId {
        &self.id
    }
    fn prepare(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn apply(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn verify(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
    fn rollback(&mut self, _operation: &Operation) -> Result<(), AdapterFailure> {
        Ok(())
    }
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
        self.fail_if(&self.fail_apply_for, operation)?;
        self.observations.lock().unwrap().insert(
            operation.resource.resource_id.to_string(),
            RecoveryObservation::After,
        );
        Ok(())
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("verify", operation);
        self.fail_if(&self.fail_verify_for, operation)
    }

    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("rollback", operation);
        self.fail_if(&self.fail_rollback_for, operation)
    }

    fn supports_recovery(&self, capability: RecoveryCapability) -> bool {
        capability == RecoveryCapability::ExactRollback || self.supports_forward
    }

    fn supports_operation(&self, operation: &Operation) -> bool {
        self.supports_recovery(operation.recovery_capability)
            && self.unsupported_resource.as_deref() != Some(operation.resource.resource_id.as_str())
    }

    fn observe_recovery(
        &mut self,
        operation: &Operation,
    ) -> Result<RecoveryObservation, AdapterFailure> {
        self.record("observe", operation);
        Ok(self
            .observations
            .lock()
            .unwrap()
            .get(operation.resource.resource_id.as_str())
            .copied()
            .unwrap_or(RecoveryObservation::Before))
    }

    fn supports_offline_recovery(&self, _operation: &Operation) -> bool {
        self.supports_forward
    }

    fn prepare_recovery(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("recovery_prepare", operation);
        Ok(())
    }

    fn converge_recovery(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.record("converge", operation);
        self.fail_if(&self.fail_apply_for, operation)?;
        self.observations.lock().unwrap().insert(
            operation.resource.resource_id.to_string(),
            RecoveryObservation::After,
        );
        Ok(())
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
        supports_forward: false,
        unsupported_resource: None,
        observations: Arc::new(Mutex::new(BTreeMap::new())),
    }
}

fn plan() -> commonkit_contracts::Plan {
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations: vec![operation("files", "alpha"), operation("files", "beta")],
    })
    .expect("plan")
}

fn plan_with(operations: Vec<Operation>) -> commonkit_contracts::Plan {
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations,
    })
    .expect("plan")
}

fn store_snapshot(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn visit(root: &std::path::Path, path: &std::path::Path, output: &mut Vec<(String, Vec<u8>)>) {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                visit(root, &entry, output);
            } else {
                output.push((
                    entry
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&entry).unwrap(),
                ));
            }
        }
    }
    let mut output = Vec::new();
    visit(root, root, &mut output);
    output
}

fn persist_crash_after_forward_barrier(
    store: &ReceiptStore,
    plan: &commonkit_contracts::Plan,
    run_id: &StableId,
) {
    let mut journal = ReceiptJournal::for_plan(run_id.clone(), plan).unwrap();
    store.persist(&journal).unwrap();
    for operation in &plan.operations {
        journal
            .record_operation(operation.id.clone(), OperationPhase::Prepared, None)
            .unwrap();
        store.persist(&journal).unwrap();
    }
    journal.transition(ReceiptState::Applying).unwrap();
    store.persist(&journal).unwrap();
    let exact = plan
        .operations
        .iter()
        .find(|operation| operation.recovery_capability == RecoveryCapability::ExactRollback)
        .unwrap();
    for phase in [
        OperationPhase::ApplyStarted,
        OperationPhase::Applied,
        OperationPhase::Verified,
    ] {
        journal
            .record_operation(exact.id.clone(), phase, None)
            .unwrap();
        store.persist(&journal).unwrap();
    }
    let forward = plan
        .operations
        .iter()
        .find(|operation| operation.recovery_capability == RecoveryCapability::ConvergeForwardOnly)
        .unwrap();
    journal
        .record_operation(forward.id.clone(), OperationPhase::ApplyStarted, None)
        .unwrap();
    store.persist(&journal).unwrap();
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
fn rolls_back_started_operations_in_reverse_order_after_apply_failure() {
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
            "rollback:beta",
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
        plan.bindings.clone(),
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
            .record_operation(operation.id.clone(), OperationPhase::ApplyStarted, None)
            .expect("apply started");
        store.persist(&journal).expect("apply-started checkpoint");
        journal
            .record_operation(operation.id.clone(), OperationPhase::Applied, None)
            .expect("applied");
        store.persist(&journal).expect("applied checkpoint");
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(adapter(events.clone()))];
    let mut legacy_value = serde_json::to_value(&plan).unwrap();
    for operation in legacy_value["operations"].as_array_mut().unwrap() {
        operation
            .as_object_mut()
            .unwrap()
            .remove("recoveryCapability");
    }
    let legacy_plan: commonkit_contracts::Plan = serde_json::from_value(legacy_value).unwrap();
    assert_eq!(legacy_plan.id, plan.id);
    let outcome = Reconciler::with_store(&store)
        .recover_run(run_id.clone(), &legacy_plan, &mut adapters)
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

#[test]
fn adapter_capability_preflight_fails_before_creating_a_receipt() {
    let directory = temporary_directory("capability-preflight");
    let store = ReceiptStore::open(&directory).unwrap();
    let forward = plan_with(vec![operation_with_capability(
        "files",
        "package",
        RecoveryCapability::ConvergeForwardOnly,
    )]);
    let before = store_snapshot(&directory);
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(adapter(events))];

    assert!(matches!(
        Reconciler::with_store(&store).execute(
            &forward,
            StableId::parse("unsupported-capability").unwrap(),
            &mut adapters,
        ),
        Err(ReconcileError::AdapterCapabilityUnsupported { .. })
    ));
    assert_eq!(store_snapshot(&directory), before);

    let mut wrong_type = adapter(Arc::new(Mutex::new(Vec::new())));
    wrong_type.unsupported_resource = Some("alpha".into());
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(wrong_type)];
    assert!(matches!(
        Reconciler::with_store(&store).execute(
            &plan(),
            StableId::parse("unsupported-type").unwrap(),
            &mut adapters,
        ),
        Err(ReconcileError::AdapterCapabilityUnsupported { .. })
    ));
    assert_eq!(store_snapshot(&directory), before);

    let mut no_adapters = Vec::<Box<dyn Adapter>>::new();
    assert!(matches!(
        Reconciler::with_store(&store).execute(
            &plan(),
            StableId::parse("missing-adapter").unwrap(),
            &mut no_adapters,
        ),
        Err(ReconcileError::AdapterNotFound(_))
    ));
    assert_eq!(store_snapshot(&directory), before);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn adapter_capability_defaults_fail_closed_for_exact_rollback() {
    let directory = temporary_directory("implicit-capability");
    let store = ReceiptStore::open(&directory).unwrap();
    let before = store_snapshot(&directory);
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(ImplicitCapabilityAdapter {
        id: StableId::parse("files").unwrap(),
    })];

    assert!(matches!(
        Reconciler::with_store(&store).execute(
            &plan(),
            StableId::parse("implicit-exact").unwrap(),
            &mut adapters,
        ),
        Err(ReconcileError::AdapterCapabilityUnsupported { .. })
    ));
    assert_eq!(store_snapshot(&directory), before);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn forward_barrier_never_rolls_back_and_records_forward_recovery_required() {
    let directory = temporary_directory("forward-barrier");
    let store = ReceiptStore::open(&directory).unwrap();
    let mixed = plan_with(vec![
        operation_with_capability("files", "exact", RecoveryCapability::ExactRollback),
        operation_with_capability("files", "forward", RecoveryCapability::ConvergeForwardOnly),
    ]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut failing = adapter(events.clone());
    failing.supports_forward = true;
    failing.fail_apply_for = Some("forward".into());
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(failing)];
    let run_id = StableId::parse("forward-failure").unwrap();

    assert_eq!(
        Reconciler::with_store(&store)
            .execute(&mixed, run_id.clone(), &mut adapters)
            .unwrap(),
        ReconcileOutcome::ForwardRecoveryRequired
    );
    assert_eq!(
        *events.lock().unwrap(),
        [
            "prepare:exact",
            "prepare:forward",
            "apply:exact",
            "verify:exact",
            "apply:forward",
        ]
    );
    let receipt = store.load(run_id.clone()).unwrap();
    assert_eq!(receipt.receipt().schema_version.0, 2);
    assert_eq!(receipt.receipt().contract_version, "2.0");
    assert_eq!(
        receipt.receipt().state,
        ReceiptState::ForwardRecoveryRequired
    );
    let mut stages = receipt
        .receipt()
        .transitions
        .iter()
        .map(|transition| transition.state)
        .collect::<Vec<_>>();
    stages.dedup();
    assert_eq!(
        stages,
        [
            ReceiptState::Prepared,
            ReceiptState::Applying,
            ReceiptState::Verifying,
            ReceiptState::ApplyingForward,
            ReceiptState::ForwardRecoveryRequired,
        ]
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_rollback_rejects_forward_only_plans_before_loading_a_receipt() {
    let directory = temporary_directory("rollback-unsupported");
    let store = ReceiptStore::open(&directory).unwrap();
    let seed = ReceiptJournal::new(
        StableId::parse("existing-run").unwrap(),
        digest('a'),
        StableId::parse("laptop").unwrap(),
        digest('b'),
        digest('c'),
        digest('d'),
        bindings(),
    )
    .unwrap();
    store.persist(&seed).unwrap();
    let before = store_snapshot(&directory);
    let forward = plan_with(vec![operation_with_capability(
        "files",
        "package",
        RecoveryCapability::ConvergeForwardOnly,
    )]);
    let mut supported = adapter(Arc::new(Mutex::new(Vec::new())));
    supported.supports_forward = true;
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(supported)];

    assert!(matches!(
        Reconciler::with_store(&store).rollback_succeeded_run(
            StableId::parse("receipt-does-not-exist").unwrap(),
            &forward,
            &mut adapters,
        ),
        Err(ReconcileError::RollbackUnsupported)
    ));
    assert_eq!(store_snapshot(&directory), before);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restart_after_forward_barrier_converges_bound_before_and_after_states_without_rollback() {
    let directory = temporary_directory("forward-restart");
    let store = ReceiptStore::open(&directory).unwrap();
    let mixed = plan_with(vec![
        operation_with_capability("files", "exact", RecoveryCapability::ExactRollback),
        operation_with_capability("files", "forward", RecoveryCapability::ConvergeForwardOnly),
    ]);
    let run_id = StableId::parse("forward-crashed").unwrap();
    persist_crash_after_forward_barrier(&store, &mixed, &run_id);

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut recovering = adapter(events.clone());
    recovering.supports_forward = true;
    recovering
        .observations
        .lock()
        .unwrap()
        .insert("exact".into(), RecoveryObservation::After);
    recovering
        .observations
        .lock()
        .unwrap()
        .insert("forward".into(), RecoveryObservation::Before);
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(recovering)];

    assert_eq!(
        Reconciler::with_store(&store)
            .recover_run(run_id.clone(), &mixed, &mut adapters)
            .unwrap(),
        ReconcileOutcome::ForwardRecovered
    );
    assert_eq!(
        *events.lock().unwrap(),
        [
            "observe:forward",
            "recovery_prepare:forward",
            "converge:forward",
            "observe:forward",
        ]
    );
    let receipt = store.load(run_id).unwrap();
    assert_eq!(receipt.receipt().state, ReceiptState::ForwardRecovered);
    assert_eq!(
        receipt
            .receipt()
            .operation_progress
            .iter()
            .find(|progress| progress.operation_id == mixed.operations[0].id)
            .unwrap()
            .phase,
        OperationPhase::Verified
    );
    assert_eq!(
        receipt
            .receipt()
            .operation_progress
            .iter()
            .find(|progress| progress.operation_id == mixed.operations[1].id)
            .unwrap()
            .phase,
        OperationPhase::ForwardRecovered
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restart_after_forward_barrier_never_forward_converges_exact_before_state() {
    let directory = temporary_directory("forward-restart-exact-before");
    let store = ReceiptStore::open(&directory).unwrap();
    let mixed = plan_with(vec![
        operation_with_capability("files", "exact", RecoveryCapability::ExactRollback),
        operation_with_capability("files", "forward", RecoveryCapability::ConvergeForwardOnly),
    ]);
    let run_id = StableId::parse("forward-exact-before").unwrap();
    persist_crash_after_forward_barrier(&store, &mixed, &run_id);

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut recovering = adapter(events.clone());
    recovering.supports_forward = true;
    recovering
        .observations
        .lock()
        .unwrap()
        .insert("exact".into(), RecoveryObservation::Before);
    recovering
        .observations
        .lock()
        .unwrap()
        .insert("forward".into(), RecoveryObservation::Before);
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(recovering)];

    assert_eq!(
        Reconciler::with_store(&store)
            .recover_run(run_id.clone(), &mixed, &mut adapters)
            .unwrap(),
        ReconcileOutcome::ForwardRecovered
    );
    assert_eq!(
        *events.lock().unwrap(),
        [
            "observe:forward",
            "recovery_prepare:forward",
            "converge:forward",
            "observe:forward",
        ]
    );
    let receipt = store.load(run_id).unwrap();
    assert_eq!(receipt.receipt().state, ReceiptState::ForwardRecovered);
    assert_eq!(
        receipt
            .receipt()
            .operation_progress
            .iter()
            .find(|progress| progress.operation_id == mixed.operations[0].id)
            .unwrap()
            .phase,
        OperationPhase::Verified
    );
    assert_eq!(
        receipt
            .receipt()
            .operation_progress
            .iter()
            .find(|progress| progress.operation_id == mixed.operations[1].id)
            .unwrap()
            .phase,
        OperationPhase::ForwardRecovered
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ambiguous_forward_recovery_fails_without_apply_or_rollback() {
    let directory = temporary_directory("forward-ambiguous");
    let store = ReceiptStore::open(&directory).unwrap();
    let mixed = plan_with(vec![
        operation_with_capability("files", "exact", RecoveryCapability::ExactRollback),
        operation_with_capability("files", "forward", RecoveryCapability::ConvergeForwardOnly),
    ]);
    let run_id = StableId::parse("forward-ambiguous").unwrap();
    persist_crash_after_forward_barrier(&store, &mixed, &run_id);

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut recovering = adapter(events.clone());
    recovering.supports_forward = true;
    recovering
        .observations
        .lock()
        .unwrap()
        .insert("exact".into(), RecoveryObservation::After);
    recovering
        .observations
        .lock()
        .unwrap()
        .insert("forward".into(), RecoveryObservation::Other);
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(recovering)];

    assert_eq!(
        Reconciler::with_store(&store)
            .recover_run(run_id.clone(), &mixed, &mut adapters)
            .unwrap(),
        ReconcileOutcome::ForwardRecoveryFailed
    );
    assert_eq!(*events.lock().unwrap(), ["observe:forward"]);
    assert_eq!(
        store.load(run_id.clone()).unwrap().receipt().state,
        ReceiptState::ForwardRecoveryFailed
    );

    let retry_events = Arc::new(Mutex::new(Vec::new()));
    let mut retrying = adapter(retry_events.clone());
    retrying.supports_forward = true;
    retrying
        .observations
        .lock()
        .unwrap()
        .insert("exact".into(), RecoveryObservation::After);
    retrying
        .observations
        .lock()
        .unwrap()
        .insert("forward".into(), RecoveryObservation::After);
    let mut retry_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(retrying)];
    assert_eq!(
        Reconciler::with_store(&store)
            .recover_run(run_id.clone(), &mixed, &mut retry_adapters)
            .unwrap(),
        ReconcileOutcome::ForwardRecovered
    );
    assert_eq!(
        store.load(run_id).unwrap().receipt().state,
        ReceiptState::ForwardRecovered
    );
    fs::remove_dir_all(directory).unwrap();
}
