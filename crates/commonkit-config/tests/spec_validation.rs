mod support;

use commonkit_config::{LayerSet, LayerSetError};
use commonkit_contracts::LayerKind;
use support::layer;

#[test]
fn rejects_unknown_commonkit_owned_spec_fields() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({"securityPolciy": {}}),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    assert!(matches!(
        LayerSet::new(vec![public, organization]),
        Err(LayerSetError::UnknownSpecField { field }) if field == "securityPolciy"
    ));
}

#[test]
fn accepts_current_v1_fields_and_provider_owned_adapter_payloads() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "theme": "dark",
            "capabilities": {
                "agentContext": {
                    "provider": "apm",
                    "version": "0.25.0",
                    "manifest": "./apm.yml",
                    "lockfile": "./apm.lock.yaml",
                    "policy": "./apm-policy.yml",
                    "targets": ["claude", "codex"]
                }
            },
            "requirements": ["git"],
            "denials": ["insecure_transport"],
            "files": [{"path":"home/editor.conf","source":"portable/editor.conf"}],
            "adapters": [{
                "id":"codex",
                "settings":{"provider.example/approvalMode":"on-request"}
            }],
            "hooks": [{"id":"session-end","provider.example/payload":{"event":"stop"}}],
            "plugins": [{"id":"review","provider.example/config":{"strict":true}}],
            "targets": [{"id":"workstation","provider.example/transport":{"kind":"local"}}]
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({
            "securityPolicy": {
                "deniedPaths": [],
                "requiredControls": {},
                "allowlists": {},
                "minimums": {},
                "maximums": {}
            }
        }),
    );

    LayerSet::new(vec![public, organization]).expect("valid current v1 layer fields");
}

#[test]
fn rejects_ambiguous_ids_within_the_first_layer() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "adapters": [
                {"id":"codex","enabled":true},
                {"id":"codex","enabled":false}
            ]
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    assert!(matches!(
        LayerSet::new(vec![public, organization]),
        Err(LayerSetError::DuplicateSpecItemId { field, id })
            if field == "adapters" && id == "codex"
    ));
}

#[test]
fn rejects_unsupported_styleguide_activation_before_composition() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "capabilities": {
                "styleguide": {
                    "skillId": "technical-writing",
                    "activation": "always_on"
                }
            }
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    assert!(matches!(
        LayerSet::new(vec![public, organization]),
        Err(LayerSetError::InvalidStyleguideSelection(_))
    ));
}

#[test]
fn rejects_multiple_active_styleguides_before_composition() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "capabilities": {
                "styleguide": [
                    {"skillId": "technical-writing", "activation": "routed"},
                    {"skillId": "other-writing", "activation": "routed"}
                ]
            }
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    assert!(matches!(
        LayerSet::new(vec![public, organization]),
        Err(LayerSetError::InvalidStyleguideSelection(_))
    ));
}
