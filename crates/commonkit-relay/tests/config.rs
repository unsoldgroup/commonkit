use commonkit_relay::{DEFAULT_TOOLS_TTL_MS, ManagedBy, RelayConfig, RelayConfigError, RelayMode};
use serde_json::json;

#[test]
fn normalizes_minimal_legacy_config_without_self_upgrade() {
    let config = RelayConfig::normalize(json!({
        "updates": {
            "autoUpgrade": true,
            "packageManager": "pnpm"
        },
        "servers": [{
            "id": "posthog",
            "mode": "posthog-cli",
            "remote": {
                "type": "streamable_http",
                "url": "https://mcp.posthog.com/mcp"
            }
        }]
    }))
    .expect("config");
    assert_eq!(config.listen.host, "127.0.0.1");
    assert_eq!(config.listen.port, 3764);
    assert_eq!(config.listen.path, "/mcp");
    assert_eq!(config.servers[0].mode, RelayMode::PosthogCli);
    assert!(config.servers[0].enabled);
    assert_eq!(config.servers[0].cache.tools_ttl_ms, DEFAULT_TOOLS_TTL_MS);
    assert_eq!(
        config.servers[0].cache.auto_refresh_ms,
        DEFAULT_TOOLS_TTL_MS
    );
    assert_eq!(config.servers[0].managed_by, ManagedBy::CommonKit);
}

#[test]
fn normalizes_refresh_disable_and_legacy_admin_fields() {
    let config = RelayConfig::normalize(json!({
        "admin": {"host": "::1", "port": 4000, "mcpPath": "/relay"},
        "servers": [{
            "id": "manual",
            "remote": {"type": "streamable_http", "url": "https://example.com/mcp"},
            "cache": {"toolsTtlMs": 60000, "autoRefreshMs": 0}
        }]
    }))
    .expect("config");
    assert_eq!(config.listen.host, "::1");
    assert_eq!(config.listen.port, 4000);
    assert_eq!(config.listen.path, "/relay");
    assert_eq!(config.servers[0].cache.tools_ttl_ms, 60_000);
    assert_eq!(config.servers[0].cache.auto_refresh_ms, 0);
}

#[test]
fn rejects_invalid_duplicate_and_non_loopback_configuration() {
    assert_eq!(
        RelayConfig::normalize(json!({
            "listen": {"host": "0.0.0.0"},
            "servers": []
        }))
        .expect_err("listen"),
        RelayConfigError::NonLoopbackListen
    );
    assert_eq!(
        RelayConfig::normalize(json!({
            "servers": [
                {"id": "same", "remote": {"type": "streamable_http", "url": "https://example.com/a"}},
                {"id": "same", "remote": {"type": "streamable_http", "url": "https://example.com/b"}}
            ]
        }))
        .expect_err("duplicate"),
        RelayConfigError::DuplicateServerId
    );
    assert!(RelayConfig::normalize(json!({
        "servers": [{"id": "../bad", "remote": {"type": "streamable_http", "url": "https://example.com"}}]
    })).is_err());
}

#[test]
fn accepts_only_secret_references_for_remote_headers() {
    let config = RelayConfig::normalize(json!({
        "servers": [{
            "id": "docs",
            "remote": {
                "type": "streamable_http",
                "url": "https://example.com/mcp",
                "headers": {"Authorization": "env:DOCS_TOKEN"}
            }
        }]
    }))
    .expect("reference");
    assert_eq!(
        config.servers[0].remote.headers["Authorization"],
        "env:DOCS_TOKEN"
    );

    assert_eq!(
        RelayConfig::normalize(json!({
            "servers": [{
                "id": "docs",
                "remote": {
                    "type": "streamable_http",
                    "url": "https://example.com/mcp",
                    "headers": {"Authorization": "Bearer literal-secret"}
                }
            }]
        }))
        .expect_err("literal"),
        RelayConfigError::LiteralHeaderSecret
    );
}

#[test]
fn preserves_local_menu_shape_but_rejects_network_actions() {
    let config = RelayConfig::normalize(json!({
        "servers": [{
            "id": "mail",
            "remote": {"type": "streamable_http", "url": "https://example.com/mcp"},
            "menu": {
                "statusUrl": "http://127.0.0.1:3765/status",
                "ttlMs": 5000,
                "actions": [{
                    "id": "sync_now",
                    "label": "Sync Now",
                    "method": "POST",
                    "url": "http://127.0.0.1:3765/sync",
                    "confirm": true
                }]
            }
        }]
    }))
    .expect("menu");
    assert_eq!(config.servers[0].menu.as_ref().expect("menu").ttl_ms, 5000);

    assert!(
        RelayConfig::normalize(json!({
            "servers": [{
                "id": "bad",
                "remote": {"type": "streamable_http", "url": "https://example.com/mcp"},
                "menu": {"statusUrl": "http://example.com/status"}
            }]
        }))
        .is_err()
    );
}
