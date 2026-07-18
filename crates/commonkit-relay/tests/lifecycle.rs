use commonkit_relay::{
    ManagedBy, RelayCatalog, RelayConfig, RelayLifecycleError, RelayRuntimeServer,
    RelayServerConfig, RelayTool, UpstreamValidator,
};
use serde_json::json;

fn server(id: &str, managed_by: ManagedBy) -> RelayServerConfig {
    let value = json!({
        "servers": [{
            "id": id,
            "managedBy": match managed_by {
                ManagedBy::CommonKit => "common_kit",
                ManagedBy::Unmanaged => "unmanaged",
            },
            "remote": {"type": "streamable_http", "url": format!("https://{id}.example/mcp")}
        }]
    });
    RelayConfig::normalize(value)
        .expect("config")
        .servers
        .remove(0)
}

fn tool(name: &str) -> RelayTool {
    RelayTool {
        name: name.into(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
    }
}

struct Validator {
    fail: Option<String>,
    tools: Vec<RelayTool>,
}

impl UpstreamValidator for Validator {
    fn discover(&self, server: &RelayServerConfig) -> Result<Vec<RelayTool>, RelayLifecycleError> {
        if self.fail.as_deref() == Some(server.id.as_str()) {
            Err(RelayLifecycleError::Validation("injected".into()))
        } else {
            Ok(self.tools.clone())
        }
    }
}

#[test]
fn validates_the_entire_desired_set_before_committing_any_change() {
    let original = RelayRuntimeServer {
        config: server("original", ManagedBy::CommonKit),
        tools: vec![tool("old")],
    };
    let mut catalog = RelayCatalog::from_servers(vec![original.clone()]).expect("catalog");
    let result = catalog.reconcile(
        vec![
            server("first", ManagedBy::CommonKit),
            server("broken", ManagedBy::CommonKit),
        ],
        true,
        &Validator {
            fail: Some("broken".into()),
            tools: vec![tool("new")],
        },
    );
    assert!(result.is_err());
    assert_eq!(catalog.servers().collect::<Vec<_>>(), [&original]);
}

#[test]
fn preserves_unmanaged_servers_while_pruning_absent_commonkit_servers() {
    let unmanaged = RelayRuntimeServer {
        config: server("local", ManagedBy::Unmanaged),
        tools: vec![tool("local")],
    };
    let managed = RelayRuntimeServer {
        config: server("old", ManagedBy::CommonKit),
        tools: vec![tool("old")],
    };
    let mut catalog = RelayCatalog::from_servers(vec![unmanaged, managed]).expect("catalog");
    let summary = catalog
        .reconcile(
            vec![server("new", ManagedBy::CommonKit)],
            true,
            &Validator {
                fail: None,
                tools: vec![tool("new")],
            },
        )
        .expect("reconcile");
    assert_eq!(summary.removed[0].as_str(), "old");
    assert_eq!(
        catalog
            .servers()
            .map(|server| server.config.id.as_str())
            .collect::<Vec<_>>(),
        ["local", "new"]
    );
}

#[test]
fn preserves_localization_and_rejects_non_injective_collisions() {
    let configured = server("docs", ManagedBy::CommonKit);
    let mut catalog = RelayCatalog::default();
    let collision = catalog.reconcile(
        vec![configured.clone()],
        false,
        &Validator {
            fail: None,
            tools: vec![tool("find.page"), tool("find/page")],
        },
    );
    assert_eq!(
        collision.expect_err("collision"),
        RelayLifecycleError::ToolNameCollision("docs__find_page".into())
    );

    catalog
        .reconcile(
            vec![configured],
            false,
            &Validator {
                fail: None,
                tools: vec![tool("find.page")],
            },
        )
        .expect("reconcile");
    let localized = catalog.localized_tools().expect("tools");
    assert_eq!(localized[0].name, "docs__find_page");
    assert_eq!(localized[0].description, "[docs] find.page");
}
