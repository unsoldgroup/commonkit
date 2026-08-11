use commonkit_contracts::{
    Operation, OperationKind, PackageConsent, PackageOperationConsentBinding, Plan, PlanBindings,
    RecoveryCapability, ResourceRef, Risk, SchemaVersion, Sha256Digest, StableId,
    package_operation_set_digest,
};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn package_operation(id: char, after: char) -> Operation {
    Operation {
        id: digest(id),
        adapter_id: StableId::parse("packages").unwrap(),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("package").unwrap(),
            resource_id: StableId::parse(format!("package-{id}")).unwrap(),
            managed_path: None,
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        recovery_capability: RecoveryCapability::ConvergeForwardOnly,
        depends_on: Vec::new(),
        before_digest: None,
        after_digest: Some(digest(after)),
        payload_digest: digest('f'),
        provenance: None,
        summary: "install exact package closure".into(),
    }
}

fn plan(operations: Vec<Operation>) -> Plan {
    Plan {
        schema_version: SchemaVersion(2),
        contract_version: "2.0".into(),
        id: digest('1'),
        target_id: StableId::parse("laptop").unwrap(),
        desired_digest: digest('2'),
        observed_digest: digest('3'),
        policy_digest: digest('4'),
        bindings: PlanBindings {
            target_identity_digest: digest('5'),
            composed_loadout_digest: digest('6'),
            provider_inputs_digest: digest('7'),
            ownership_map_digest: digest('8'),
            artifact_set_digest: digest('9'),
            package_resolution_authority_digest: Some(digest('a')),
        },
        operations,
    }
}

#[test]
fn package_consent_binds_the_exact_sorted_forward_operation_set() {
    let first = package_operation('b', 'c');
    let second = package_operation('a', 'd');
    let reviewed = plan(vec![first.clone(), second.clone()]);
    let bindings = vec![
        PackageOperationConsentBinding {
            operation_id: first.id.clone(),
            resolution_digest: digest('e'),
        },
        PackageOperationConsentBinding {
            operation_id: second.id.clone(),
            resolution_digest: digest('f'),
        },
    ];

    let operation_set_digest = package_operation_set_digest(&reviewed, &bindings).unwrap();
    let reversed = bindings.iter().cloned().rev().collect::<Vec<_>>();
    assert_eq!(
        package_operation_set_digest(&reviewed, &reversed).unwrap(),
        operation_set_digest,
    );

    let consent = PackageConsent {
        confirmation_id: StableId::parse("package-review").unwrap(),
        operation_set_digest: operation_set_digest.clone(),
    };
    consent.validate(&reviewed, &bindings).unwrap();

    let mut changed = bindings;
    changed[0].resolution_digest = digest('0');
    assert!(consent.validate(&reviewed, &changed).is_err());

    let changed_plan = plan(vec![second]);
    assert!(consent.validate(&changed_plan, &changed[1..]).is_err());
}

#[test]
fn package_consent_rejects_non_additive_or_exact_rollback_operations() {
    let binding = |operation: &Operation| PackageOperationConsentBinding {
        operation_id: operation.id.clone(),
        resolution_digest: digest('e'),
    };

    let mut exact = package_operation('a', 'b');
    exact.recovery_capability = RecoveryCapability::ExactRollback;
    assert!(package_operation_set_digest(&plan(vec![exact.clone()]), &[binding(&exact)]).is_err());

    let mut destructive = package_operation('c', 'd');
    destructive.kind = OperationKind::Delete;
    assert!(
        package_operation_set_digest(&plan(vec![destructive.clone()]), &[binding(&destructive)])
            .is_err()
    );

    let mut preimage = package_operation('e', 'f');
    preimage.before_digest = Some(digest('0'));
    assert!(
        package_operation_set_digest(&plan(vec![preimage.clone()]), &[binding(&preimage)]).is_err()
    );
}
