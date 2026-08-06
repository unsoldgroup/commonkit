mod support;

use commonkit_config::{LayerSet, MergeRules, compose_layers, v1_merge_rules};
use commonkit_contracts::{LayerKind, MergeOperation};
use support::layer;

#[test]
fn records_winner_contributions_and_a_reproducible_lockfile() {
    let set = LayerSet::new(vec![
        layer(
            "base",
            LayerKind::PublicBase,
            serde_json::json!({"files": [{"path": "base", "source": "base"}]}),
        ),
        layer("org", LayerKind::OrganizationPolicy, serde_json::json!({})),
        layer(
            "personal",
            LayerKind::PersonalKit,
            serde_json::json!({"files": [{"path": "personal", "source": "personal"}]}),
        ),
        layer(
            "target",
            LayerKind::TargetOverrides,
            serde_json::json!({"files": [{"path": "target", "source": "target"}]}),
        ),
    ])
    .expect("layers");

    let result = compose_layers(&set, &MergeRules::new()).expect("composition");
    assert_eq!(result.spec["files"][0]["path"], "target");
    assert_eq!(result.trace.state_digest, result.spec_digest);

    let files = result.trace.entries.get("/files").expect("files trace");
    assert_eq!(files.winner.layer_id.as_str(), "target");
    assert_eq!(files.winner.operation, MergeOperation::Replace);
    assert_eq!(
        files
            .contributions
            .iter()
            .map(|entry| entry.layer_id.as_str())
            .collect::<Vec<_>>(),
        vec!["base", "personal", "target"]
    );
    assert_eq!(
        result
            .lock
            .layers
            .iter()
            .map(|layer| layer.kind)
            .collect::<Vec<_>>(),
        vec![
            LayerKind::PublicBase,
            LayerKind::OrganizationPolicy,
            LayerKind::PersonalKit,
            LayerKind::TargetOverrides,
        ]
    );
    assert_eq!(result.lock.normalized_digest, result.spec_digest);
}

#[test]
fn composes_the_complete_five_layer_v1_contract_with_provenance() {
    let set = LayerSet::new(vec![
        layer(
            "base",
            LayerKind::PublicBase,
            serde_json::json!({
                "contextBudget": {"maxTotalTokens": 100},
                "files": [{"path": "base", "source": "base"}],
                "capabilities": {"styleguide": {"skillId": "base-writing", "activation": "routed"}}
            }),
        ),
        layer(
            "org",
            LayerKind::OrganizationPolicy,
            serde_json::json!({
                "securityPolicy": {"deniedPaths": ["**/.env"]}
            }),
        ),
        layer(
            "personal",
            LayerKind::PersonalKit,
            serde_json::json!({
                "contextBudget": {"maxTotalTokens": 90},
                "files": [{"path": "personal", "source": "personal"}]
            }),
        ),
        layer(
            "project",
            LayerKind::ProjectLoadout,
            serde_json::json!({
                "capabilities": {"styleguide": {"skillId": "project-writing", "activation": "routed"}}
            }),
        ),
        layer(
            "target",
            LayerKind::TargetOverrides,
            serde_json::json!({
                "contextBudget": {"maxTotalTokens": 80}
            }),
        ),
    ])
    .expect("complete layer set");

    let result = compose_layers(&set, &v1_merge_rules()).expect("five-layer composition");
    assert_eq!(result.spec["contextBudget"]["maxTotalTokens"], 80);
    assert_eq!(result.spec["files"][0]["path"], "personal");
    assert_eq!(result.spec["securityPolicy"]["deniedPaths"][0], "**/.env");
    assert_eq!(
        result.spec["capabilities"]["styleguide"]["skillId"],
        "project-writing"
    );
    assert_eq!(
        result
            .trace
            .entries
            .get("/files")
            .expect("files provenance")
            .contributions
            .iter()
            .map(|entry| entry.layer_id.as_str())
            .collect::<Vec<_>>(),
        vec!["base", "personal"]
    );
    assert_eq!(result.lock.layers.len(), 5);
}
