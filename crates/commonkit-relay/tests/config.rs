use commonkit_relay::{DEFAULT_TOOLS_TTL_MS, ManagedBy, RelayConfig, RelayConfigError, RelayMode};
use serde_json::json;
use std::process::Command;

#[test]
fn preserved_node_fixture_has_normalized_rust_parity() {
    let legacy = serde_json::from_str(include_str!("fixtures/node-legacy-config.json"))
        .expect("legacy fixture");
    let expected: RelayConfig =
        serde_json::from_str(include_str!("fixtures/node-legacy-expected.json"))
            .expect("expected fixture");
    assert_eq!(
        RelayConfig::normalize(legacy).expect("normalized"),
        expected
    );
}

#[test]
fn live_preserved_node_normalizer_has_semantic_parity() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let module = root.join("packages/mcp-local-relay/dist/src/config.js");
    if !module.is_file() {
        return;
    }
    let fixture = root.join("crates/commonkit-relay/tests/fixtures/node-legacy-config.json");
    let script = r#"import {readFile} from 'node:fs/promises'; import {normalizeConfig} from './packages/mcp-local-relay/dist/src/config.js'; const raw=JSON.parse(await readFile(process.argv[1],'utf8')); process.stdout.write(JSON.stringify(normalizeConfig(raw)));"#;
    let output = Command::new("node")
        .current_dir(&root)
        .args([
            "--input-type=module",
            "--eval",
            script,
            fixture.to_str().expect("fixture path"),
        ])
        .output()
        .expect("run preserved Node normalizer");
    assert!(output.status.success(), "preserved Node normalizer failed");
    let node_value = serde_json::from_slice(&output.stdout).expect("Node output");
    let rust_from_node = RelayConfig::normalize(node_value).expect("normalize Node output");
    let rust_direct = RelayConfig::normalize(
        serde_json::from_str(include_str!("fixtures/node-legacy-config.json")).expect("fixture"),
    )
    .expect("normalize fixture");
    assert_eq!(rust_from_node, rust_direct);
}

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

    for unsupported in ["bws:item", "keychain:item", "vault:item"] {
        assert_eq!(
            RelayConfig::normalize(json!({
                "servers": [{
                    "id": "docs",
                    "remote": {
                        "type": "streamable_http",
                        "url": "https://example.com/mcp",
                        "headers": {"Authorization": unsupported}
                    }
                }]
            }))
            .expect_err("unsupported reference must fail before apply"),
            RelayConfigError::LiteralHeaderSecret
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_rejects_env_files_before_runtime_without_an_acl_loader() {
    assert_eq!(
        RelayConfig::normalize(json!({
            "servers": [{
                "id": "docs",
                "envFile": "docs.env",
                "remote": {
                    "type": "streamable_http",
                    "url": "https://example.com/mcp",
                    "headers": {"Authorization": "env:DOCS_TOKEN"}
                }
            }]
        }))
        .expect_err("unsupported credential source"),
        RelayConfigError::UnsupportedEnvFilePlatform
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
