use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_relay::{
    EnvFileLoader, RelayServerId, RelayStorageError, RelayTool, ToolCache, ToolCacheStore,
};
use serde_json::json;

fn temporary_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "commonkit-relay-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[cfg(unix)]
#[test]
fn loads_legacy_env_syntax_without_exposing_values_in_errors() {
    use std::os::unix::fs::PermissionsExt;

    let root = temporary_directory("env-parse");
    fs::create_dir_all(&root).expect("root");
    let path = root.join("relay.env");
    fs::write(
        &path,
        "# ignored\n TOKEN = 'private-value'\nEMPTY=\nQUOTED=\"two words\"\ninvalid\n",
    )
    .expect("write env");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("permissions");

    let values = EnvFileLoader::new(&root)
        .expect("loader")
        .load(Some("relay.env"))
        .expect("load");
    assert_eq!(
        values.get("TOKEN").map(String::as_str),
        Some("private-value")
    );
    assert_eq!(values.get("EMPTY").map(String::as_str), Some(""));
    assert_eq!(values.get("QUOTED").map(String::as_str), Some("two words"));
    assert!(!values.contains_key("invalid"));

    fs::remove_dir_all(root).expect("cleanup");
}

fn tool(name: &str) -> RelayTool {
    RelayTool {
        name: name.into(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
    }
}

#[test]
fn tool_cache_round_trips_and_retains_stale_data_when_disk_data_is_malformed() {
    let root = temporary_directory("cache");
    let store = ToolCacheStore::open(&root).expect("store");
    let id = RelayServerId::parse("docs").expect("id");
    let expected = ToolCache {
        cached_at: 42,
        tools: vec![tool("search")],
    };
    store.save(&id, &expected).expect("save");
    assert_eq!(store.load(&id).expect("load"), Some(expected.clone()));

    fs::write(root.join("docs.tools.json"), "{bad json").expect("corrupt cache");
    let mut retained = expected.clone();
    assert!(!store.load_or_retain(&id, &mut retained).expect("retain"));
    assert_eq!(retained, expected);

    let entries = fs::read_dir(&root)
        .expect("entries")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, ["docs.tools.json"]);
    fs::remove_dir_all(root).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn env_loader_rejects_insecure_permissions_and_root_escapes_without_secret_leakage() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temporary_directory("env-security");
    let outside = temporary_directory("env-outside");
    fs::create_dir_all(&root).expect("root");
    fs::create_dir_all(&outside).expect("outside");
    let exposed = root.join("exposed.env");
    fs::write(&exposed, "TOKEN=do-not-leak").expect("write exposed");
    fs::set_permissions(&exposed, fs::Permissions::from_mode(0o640)).expect("permissions");
    let loader = EnvFileLoader::new(&root).expect("loader");
    let permission_error = loader.load(Some("exposed.env")).expect_err("insecure");
    assert!(matches!(
        permission_error,
        RelayStorageError::InsecureEnvPermissions
    ));
    assert!(!permission_error.to_string().contains("do-not-leak"));

    assert!(matches!(
        loader.load(Some("../outside.env")),
        Err(RelayStorageError::RootEscape)
    ));
    let target = outside.join("secret.env");
    fs::write(&target, "TOKEN=also-private").expect("outside env");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("permissions");
    symlink(&target, root.join("linked.env")).expect("symlink");
    assert!(matches!(
        loader.load(Some("linked.env")),
        Err(RelayStorageError::RootEscape)
    ));
    assert!(
        loader
            .load(Some("missing.env"))
            .expect("missing is empty")
            .is_empty()
    );

    fs::remove_dir_all(root).expect("cleanup root");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

#[test]
fn cache_accepts_legacy_minimal_tools_and_ignores_unreadable_shape() {
    let root = temporary_directory("legacy-cache");
    let store = ToolCacheStore::open(&root).expect("store");
    let id = RelayServerId::parse("legacy").expect("id");
    fs::write(
        root.join("legacy.tools.json"),
        r#"{"cachedAt":7,"tools":[{"name":"echo"}]}"#,
    )
    .expect("legacy cache");
    let cache = store.load(&id).expect("load").expect("cache");
    assert_eq!(cache.cached_at, 7);
    assert_eq!(cache.tools[0].name, "echo");
    assert_eq!(cache.tools[0].description, "");
    assert_eq!(cache.tools[0].input_schema, serde_json::Value::Null);
}
