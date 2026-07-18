use commonkit_config::{MergeRules, MergeStrategy, merge_specs};

#[test]
fn applies_schema_selected_merge_strategies() {
    let rules = MergeRules::new()
        .with_strategy("/requiredCommands", MergeStrategy::SetUnion)
        .with_strategy(
            "/adapters",
            MergeStrategy::MergeById {
                id_key: "id".into(),
            },
        )
        .allow_delete("/optional");
    let base = serde_json::json!({
        "theme": "light",
        "nested": {"keep": true, "replace": 1},
        "requiredCommands": ["git", "node"],
        "adapters": [
            {"id": "codex", "enabled": true, "settings": {"sandbox": "workspace"}}
        ],
        "optional": "remove-me"
    });
    let overlay = serde_json::json!({
        "theme": "dark",
        "nested": {"replace": 2},
        "requiredCommands": ["node", "cargo"],
        "adapters": [
            {"id": "codex", "settings": {"approval": "on-request"}},
            {"id": "claude", "enabled": true}
        ],
        "optional": {"$delete": true}
    });

    assert_eq!(
        merge_specs(&base, &overlay, &rules).expect("merge"),
        serde_json::json!({
            "theme": "dark",
            "nested": {"keep": true, "replace": 2},
            "requiredCommands": ["git", "node", "cargo"],
            "adapters": [
                {"id": "codex", "enabled": true, "settings": {"sandbox": "workspace", "approval": "on-request"}},
                {"id": "claude", "enabled": true}
            ]
        })
    );
}

#[test]
fn rejects_deletion_on_paths_not_enabled_by_the_schema() {
    let error = merge_specs(
        &serde_json::json!({"protected": true}),
        &serde_json::json!({"protected": {"$delete": true}}),
        &MergeRules::new(),
    )
    .expect_err("delete must fail closed");

    assert_eq!(error.pointer(), "/protected");
}
