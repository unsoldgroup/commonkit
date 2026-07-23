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
            serde_json::json!({"theme": "light"}),
        ),
        layer("org", LayerKind::OrganizationPolicy, serde_json::json!({})),
        layer(
            "personal",
            LayerKind::PersonalKit,
            serde_json::json!({"theme": "dark"}),
        ),
        layer(
            "target",
            LayerKind::TargetOverrides,
            serde_json::json!({"theme": "system"}),
        ),
    ])
    .expect("layers");

    let result = compose_layers(&set, &MergeRules::new()).expect("composition");
    assert_eq!(result.spec["theme"], "system");
    assert_eq!(result.trace.state_digest, result.spec_digest);

    let theme = result.trace.entries.get("/theme").expect("theme trace");
    assert_eq!(theme.winner.layer_id.as_str(), "target");
    assert_eq!(theme.winner.operation, MergeOperation::Replace);
    assert_eq!(
        theme
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
                "theme": "light",
                "settings": {"editor": {"font": "mono", "size": 12}},
                "requirements": ["git"],
                "arguments": ["--base"],
                "adapters": [{"id": "codex", "enabled": true}]
            }),
        ),
        layer(
            "org",
            LayerKind::OrganizationPolicy,
            serde_json::json!({
                "requirements": ["secret-scan"],
                "securityPolicy": {"deniedPaths": ["**/.env"]}
            }),
        ),
        layer(
            "personal",
            LayerKind::PersonalKit,
            serde_json::json!({
                "settings": {"editor": {"size": 14}},
                "requirements": ["git", "node"],
                "adapters": [{"id": "codex", "settings": {"approval": "on-request"}}]
            }),
        ),
        layer(
            "project",
            LayerKind::ProjectLoadout,
            serde_json::json!({
                "arguments": ["--project"],
                "adapters": [{"id": "claude", "enabled": true}]
            }),
        ),
        layer(
            "target",
            LayerKind::TargetOverrides,
            serde_json::json!({
                "theme": "system",
                "settings": {"platform": "macos"}
            }),
        ),
    ])
    .expect("complete layer set");

    let result = compose_layers(&set, &v1_merge_rules()).expect("five-layer composition");
    assert_eq!(result.spec["theme"], "system");
    assert_eq!(result.spec["settings"]["editor"]["font"], "mono");
    assert_eq!(result.spec["settings"]["editor"]["size"], 14);
    assert_eq!(result.spec["settings"]["platform"], "macos");
    assert_eq!(
        result.spec["requirements"],
        serde_json::json!(["git", "secret-scan", "node"])
    );
    assert_eq!(result.spec["arguments"], serde_json::json!(["--project"]));
    assert_eq!(result.spec["adapters"][0]["enabled"], true);
    assert_eq!(
        result.spec["adapters"][0]["settings"]["approval"],
        "on-request"
    );
    assert_eq!(result.spec["adapters"][1]["id"], "claude");
    assert_eq!(
        result
            .trace
            .entries
            .get("/theme")
            .expect("theme provenance")
            .contributions
            .iter()
            .map(|entry| entry.layer_id.as_str())
            .collect::<Vec<_>>(),
        vec!["base", "target"]
    );
    assert_eq!(result.lock.layers.len(), 5);
}
