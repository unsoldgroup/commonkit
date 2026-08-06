use std::fs;

use commonkit_contracts::{OperationKind, Sha256Digest, StableId};
use commonkit_reconcile::Adapter;
use commonkit_relay::{
    LegacyRelayReader, RelayAdapter, RelayMutationInputs, RelayPlanRequest, plan_relay_operation,
};
use serde_json::json;

fn digest(value: &str) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", value.repeat(64 / value.len()))).expect("digest")
}

fn desired() -> commonkit_relay::RelayConfig {
    commonkit_relay::RelayConfig::normalize(json!({
        "servers": [{
            "id": "docs",
            "remote": {"type": "streamable_http", "url": "https://docs.example/mcp"}
        }]
    }))
    .expect("config")
}

#[test]
fn relay_operation_is_content_addressed_bound_and_requires_confirmation() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let mut adapter = RelayAdapter::open(
        StableId::parse("relay").expect("id"),
        temporary.path().join("live.json"),
        temporary.path().join("state"),
    )
    .expect("adapter");
    let request = RelayPlanRequest {
        desired: desired(),
        inputs: RelayMutationInputs {
            provider_inputs_digest: digest("a"),
            policy_digest: digest("b"),
            target_digest: digest("c"),
            declaration_digest: digest("d"),
            ownership_map_digest: digest("e"),
            artifact_set_digest: digest("f"),
            approved_confirmation_id: StableId::parse("relay-review-one").unwrap(),
            approval_idempotency_key: "relay-request-one".into(),
        },
    };

    let first = plan_relay_operation(&mut adapter, request.clone())
        .expect("planning")
        .expect("operation");
    let repeat = plan_relay_operation(&mut adapter, request.clone())
        .expect("planning")
        .expect("operation");
    assert_eq!(first, repeat);
    assert!(first.requires_confirmation);
    assert_eq!(first.kind, OperationKind::Create);
    assert!(
        adapter
            .validate_confirmation(&first, &StableId::parse("relay-review-one").unwrap())
            .is_ok()
    );
    assert!(
        adapter
            .validate_confirmation(&first, &StableId::parse("relay-review-replayed").unwrap())
            .is_err()
    );
    assert!(
        adapter
            .validate_approval(
                &first,
                &StableId::parse("relay-review-one").unwrap(),
                "relay-request-one"
            )
            .is_ok()
    );
    assert!(
        adapter
            .validate_approval(
                &first,
                &StableId::parse("relay-review-one").unwrap(),
                "relay-request-replayed"
            )
            .is_err()
    );

    let mut changed = request;
    changed.inputs.policy_digest = digest("d");
    let rebound = plan_relay_operation(&mut adapter, changed)
        .expect("planning")
        .expect("operation");
    assert_ne!(first.id, rebound.id);
    assert_ne!(first.payload_digest, rebound.payload_digest);
}

#[test]
fn relay_adapter_applies_verifies_and_rolls_back_from_a_fresh_process() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let live = temporary.path().join("live.json");
    let state = temporary.path().join("state");
    fs::write(&live, br#"{"servers":[]}"#).expect("old config");
    let mut adapter =
        RelayAdapter::open(StableId::parse("relay").expect("id"), &live, &state).expect("adapter");
    let operation = plan_relay_operation(
        &mut adapter,
        RelayPlanRequest {
            desired: desired(),
            inputs: RelayMutationInputs {
                provider_inputs_digest: digest("a"),
                policy_digest: digest("b"),
                target_digest: digest("c"),
                declaration_digest: digest("d"),
                ownership_map_digest: digest("e"),
                artifact_set_digest: digest("f"),
                approved_confirmation_id: StableId::parse("relay-review-two").unwrap(),
                approval_idempotency_key: "relay-request-two".into(),
            },
        },
    )
    .expect("planning")
    .expect("operation");
    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    assert!(
        plan_relay_operation(
            &mut adapter,
            RelayPlanRequest {
                desired: desired(),
                inputs: RelayMutationInputs {
                    provider_inputs_digest: digest("a"),
                    policy_digest: digest("b"),
                    target_digest: digest("c"),
                    declaration_digest: digest("d"),
                    ownership_map_digest: digest("e"),
                    artifact_set_digest: digest("f"),
                    approved_confirmation_id: StableId::parse("relay-review-two").unwrap(),
                    approval_idempotency_key: "relay-request-two".into(),
                },
            },
        )
        .expect("planning")
        .is_none()
    );
    drop(adapter);

    let mut recovered =
        RelayAdapter::open(StableId::parse("relay").expect("id"), &live, &state).expect("adapter");
    recovered.rollback(&operation).expect("rollback");
    assert_eq!(fs::read(&live).expect("live"), br#"{"servers":[]}"#);
}

#[test]
fn legacy_reader_normalizes_preserved_node_configuration() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let legacy = temporary.path().join("config.json");
    fs::write(
        &legacy,
        br#"{"admin":{"host":"::1","port":4000,"mcpPath":"/relay"},"updates":{"autoUpgrade":true},"servers":[]}"#,
    )
    .expect("legacy config");

    let migrated = LegacyRelayReader::read(&legacy).expect("migrated config");
    assert_eq!(migrated.listen.host, "::1");
    assert_eq!(migrated.listen.port, 4000);
    assert_eq!(migrated.listen.path, "/relay");
}
