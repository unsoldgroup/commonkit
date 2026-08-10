use commonkit_core::{
    OperationDraft, OperationKind, PlanBindings, PlanBuildError, PlanDraft, RecoveryCapability,
    ResourceRef, Risk, Sha256Digest, StableId, build_plan, digest_domain_json, finalize_operation,
};

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("ID")
}

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn operation(adapter: &str, resource: &str, summary: &str) -> OperationDraft {
    OperationDraft {
        adapter_id: id(adapter),
        kind: OperationKind::Update,
        resource: ResourceRef {
            resource_type: id("file"),
            resource_id: id(resource),
            managed_path: None,
        },
        risk: Risk::Low,
        requires_confirmation: false,
        recovery_capability: RecoveryCapability::ExactRollback,
        depends_on: vec![],
        before_digest: Some(digest('1')),
        after_digest: Some(digest('2')),
        payload_digest: digest('3'),
        provenance: None,
        summary: summary.into(),
    }
}

fn operation_with_capability(
    adapter: &str,
    resource: &str,
    capability: RecoveryCapability,
) -> OperationDraft {
    let mut draft = operation(adapter, resource, resource);
    draft.recovery_capability = capability;
    draft
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

#[test]
fn operation_id_excludes_display_summary() {
    let left = finalize_operation(operation("codex", "settings", "Update settings")).expect("op");
    let right =
        finalize_operation(operation("codex", "settings", "Localized summary")).expect("op");
    assert_eq!(left.id, right.id);
}

#[test]
fn operation_without_provenance_retains_the_legacy_v1_identity() {
    let draft = operation("codex", "settings", "Update settings");
    let legacy = digest_domain_json(
        "commonkit.operation.v1",
        &serde_json::json!({
            "adapterId": draft.adapter_id,
            "kind": draft.kind,
            "resource": draft.resource,
            "risk": draft.risk,
            "requiresConfirmation": draft.requires_confirmation,
            "dependsOn": draft.depends_on,
            "beforeDigest": draft.before_digest,
            "afterDigest": draft.after_digest,
            "payloadDigest": draft.payload_digest,
        }),
    )
    .expect("legacy identity");

    assert_eq!(
        finalize_operation(draft).expect("operation").id,
        legacy,
        "optional provenance must not invalidate durable pre-provenance v1 plans"
    );
}

#[test]
fn plan_id_and_order_are_independent_of_input_enumeration() {
    let first = finalize_operation(operation("codex", "settings", "Codex")).expect("op");
    let second = finalize_operation(operation("claude", "settings", "Claude")).expect("op");
    let draft = |operations| PlanDraft {
        target_id: id("laptop"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations,
    };

    let left = build_plan(draft(vec![first.clone(), second.clone()])).expect("plan");
    let right = build_plan(draft(vec![second, first])).expect("plan");
    assert_eq!(left.id, right.id);
    assert_eq!(
        left.operations
            .iter()
            .map(|operation| operation.adapter_id.as_str())
            .collect::<Vec<_>>(),
        vec!["claude", "codex"]
    );

    let mut changed = draft(left.operations.clone());
    changed.desired_digest = digest('d');
    assert_ne!(left.id, build_plan(changed).expect("changed plan").id);

    let mut changed = draft(left.operations.clone());
    changed.bindings.provider_inputs_digest = digest('8');
    assert_ne!(left.id, build_plan(changed).expect("changed bindings").id);

    let mut changed = draft(left.operations.clone());
    changed.bindings.target_identity_digest = digest('9');
    assert_ne!(
        left.id,
        build_plan(changed).expect("changed target identity").id
    );
}

#[test]
fn orders_dependencies_before_dependents_and_rejects_cycles() {
    let prerequisite =
        finalize_operation(operation("zeta", "prerequisite", "First")).expect("prerequisite");
    let mut dependent_draft = operation("alpha", "dependent", "Second");
    dependent_draft.depends_on = vec![prerequisite.id.clone()];
    let dependent = finalize_operation(dependent_draft).expect("dependent");
    let draft = |operations| PlanDraft {
        target_id: id("laptop"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations,
    };
    let plan = build_plan(draft(vec![dependent.clone(), prerequisite.clone()])).expect("plan");
    assert_eq!(plan.operations[0].id, prerequisite.id);
    assert_eq!(plan.operations[1].id, dependent.id);

    let mut left = prerequisite;
    let mut right = dependent;
    left.depends_on = vec![right.id.clone()];
    right.depends_on = vec![left.id.clone()];
    assert!(matches!(
        build_plan(draft(vec![left, right])),
        Err(PlanBuildError::DependencyCycle)
    ));
}

#[test]
fn orders_exact_rollback_before_forward_only_and_binds_capability_into_identity() {
    let exact = finalize_operation(operation_with_capability(
        "zeta",
        "exact",
        RecoveryCapability::ExactRollback,
    ))
    .unwrap();
    let forward = finalize_operation(operation_with_capability(
        "alpha",
        "forward",
        RecoveryCapability::ConvergeForwardOnly,
    ))
    .unwrap();
    let plan = build_plan(PlanDraft {
        target_id: id("laptop"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations: vec![forward.clone(), exact.clone()],
    })
    .unwrap();

    assert_eq!(plan.operations[0].id, exact.id);
    assert_eq!(plan.operations[1].id, forward.id);
    let mut changed =
        operation_with_capability("zeta", "exact", RecoveryCapability::ConvergeForwardOnly);
    changed.summary = "identity must ignore only display text".into();
    assert_ne!(exact.id, finalize_operation(changed).unwrap().id);
}

#[test]
fn rejects_exact_operations_that_transitively_depend_on_forward_only_work() {
    let forward = finalize_operation(operation_with_capability(
        "alpha",
        "forward",
        RecoveryCapability::ConvergeForwardOnly,
    ))
    .unwrap();
    let mut middle =
        operation_with_capability("alpha", "middle", RecoveryCapability::ConvergeForwardOnly);
    middle.depends_on = vec![forward.id.clone()];
    let middle = finalize_operation(middle).unwrap();
    let mut exact = operation_with_capability("alpha", "exact", RecoveryCapability::ExactRollback);
    exact.depends_on = vec![middle.id.clone()];
    let exact = finalize_operation(exact).unwrap();

    assert!(matches!(
        build_plan(PlanDraft {
            target_id: id("laptop"),
            desired_digest: digest('a'),
            observed_digest: digest('b'),
            policy_digest: digest('c'),
            bindings: bindings(),
            operations: vec![exact, middle, forward],
        }),
        Err(PlanBuildError::ExactDependsOnForwardOnly(_))
    ));
}
