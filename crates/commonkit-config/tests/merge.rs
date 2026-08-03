use commonkit_config::{MergeRules, MergeStrategy, merge_specs, v1_merge_rules};

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
fn later_styleguide_layers_replace_the_complete_selection() {
    let base = serde_json::json!({
        "capabilities": {
            "styleguide": {
                "skillId": "technical-writing",
                "activation": "routed"
            }
        }
    });
    let overlay = serde_json::json!({
        "capabilities": {
            "styleguide": {
                "skillId": "organization-writing",
                "activation": "routed"
            }
        }
    });

    assert_eq!(
        merge_specs(&base, &overlay, &v1_merge_rules()).expect("merge"),
        overlay
    );
}

#[test]
fn v1_rules_merge_all_supported_fields() {
    let base = serde_json::json!({
        "securityPolicy": {"deniedPaths": ["**/.env"]},
        "contextBudget": {"maxTotalTokens": 100},
        "files": [{"path": "base", "source": "base"}],
        "capabilities": {"styleguide": {"skillId": "base-writing", "activation": "routed"}}
    });
    let overlay = serde_json::json!({
        "securityPolicy": {"deniedPaths": ["**/.ssh"]},
        "contextBudget": {"maxTotalTokens": 80},
        "files": [{"path": "project", "source": "project"}],
        "capabilities": {"styleguide": {"skillId": "project-writing", "activation": "routed"}}
    });

    assert_eq!(
        merge_specs(&base, &overlay, &v1_merge_rules()).expect("merge supported fields"),
        serde_json::json!({
            "securityPolicy": {"deniedPaths": ["**/.env", "**/.ssh"]},
            "contextBudget": {"maxTotalTokens": 80},
            "files": [{"path": "project", "source": "project"}],
            "capabilities": {"styleguide": {"skillId": "project-writing", "activation": "routed"}}
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

#[test]
fn rejects_type_changes_selected_for_recursive_merge() {
    let error = merge_specs(
        &serde_json::json!({"settings": {"enabled": true}}),
        &serde_json::json!({"settings": "replace-the-map"}),
        &MergeRules::new().with_strategy("/settings", MergeStrategy::RecursiveMap),
    )
    .expect_err("a recursive map cannot change resource type");

    assert_eq!(error.pointer(), "/settings");
}

#[test]
fn rejects_duplicate_ids_within_a_merge_by_id_layer() {
    let rules = MergeRules::new().with_strategy(
        "/adapters",
        MergeStrategy::MergeById {
            id_key: "id".into(),
        },
    );
    let error = merge_specs(
        &serde_json::json!({"adapters": []}),
        &serde_json::json!({
            "adapters": [
                {"id": "codex", "enabled": true},
                {"id": "codex", "enabled": false}
            ]
        }),
        &rules,
    )
    .expect_err("one layer cannot ambiguously declare the same stable ID twice");

    assert_eq!(error.pointer(), "/adapters");
}
