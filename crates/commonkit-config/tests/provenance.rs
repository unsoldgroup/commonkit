mod support;

use commonkit_config::{LayerSet, MergeRules, compose_layers};
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
