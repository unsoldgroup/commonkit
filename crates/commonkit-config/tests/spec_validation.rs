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
fn accepts_exactly_the_supported_v1_fields() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "capabilities": {
                "styleguide": {
                    "skillId": "technical-writing",
                    "activation": "routed"
                }
            },
            "contextBudget": {
                "maxTotalTokens": 1000
            },
            "files": [{"path":"home/editor.conf","source":"portable/editor.conf"}],
            "packages": [{"id":"ripgrep","version":"14.1.1","manager":"homebrew","source":"homebrew_core"}],
            "securityPolicy": {
                "deniedPaths": [],
                "requiredControls": {},
                "allowlists": {},
                "minimums": {},
                "maximums": {}
            }
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    LayerSet::new(vec![public, organization]).expect("supported v1 layer fields");
}

#[test]
fn rejects_malformed_packages_before_composition() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "packages": [{"id":"node","version":"^24","manager":"fnm","source":"nodejs_org"}]
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    assert!(matches!(
        LayerSet::new(vec![public, organization]),
        Err(LayerSetError::InvalidPackagesDeclaration(_))
    ));
}

#[test]
fn rejects_formerly_declared_domains_with_layer_specific_guidance() {
    let public = layer(
        "public",
        LayerKind::PublicBase,
        serde_json::json!({
            "adapters": [{"id":"codex","enabled":true}]
        }),
    );
    let organization = layer(
        "organization",
        LayerKind::OrganizationPolicy,
        serde_json::json!({}),
    );

    assert!(matches!(
        LayerSet::new(vec![public, organization]),
        Err(LayerSetError::UnsupportedLayerSpecField { field }) if field == "adapters"
    ));
}

#[test]
fn removed_domain_error_explains_that_the_feature_still_exists() {
    let result = LayerSet::new(vec![
        layer(
            "public",
            LayerKind::PublicBase,
            serde_json::json!({"credentials": {}}),
        ),
        layer(
            "organization",
            LayerKind::OrganizationPolicy,
            serde_json::json!({}),
        ),
    ]);
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("credentials are not configured through layers"),
    };

    assert_eq!(
        error.to_string(),
        "CommonKit v1 layer declaration credentials is not supported; this does not mean the feature is unavailable, only that it is configured outside layers"
    );
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
