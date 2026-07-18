use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn temporary_directory(test: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn layer(id: &str, kind: &str, spec: Value) -> Value {
    json!({
        "schemaVersion": 1,
        "id": id,
        "kind": kind,
        "source": {
            "path": format!("layers/{id}.json"),
            "revision": "57a085e7d0b558e71c8d2255b7e60e6c677dee76",
            "contentDigest": format!("sha256:{}", "0".repeat(64))
        },
        "spec": spec
    })
}

#[test]
fn reports_versioned_machine_readable_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .arg("status")
        .output()
        .expect("status");
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(status["contractVersion"], "1.0");
    assert_eq!(status["schemaVersion"], 1);
    assert!(status["stateDirectory"].as_str().is_some());
}

#[test]
fn composes_layers_and_explains_the_winning_value() {
    let directory = temporary_directory("cli-compose");
    fs::create_dir_all(&directory).expect("directory");
    let base = directory.join("base.json");
    let organization = directory.join("organization.json");
    let personal = directory.join("personal.json");
    fs::write(
        &base,
        serde_json::to_vec(&layer("base", "public_base", json!({"theme": "light"}))).expect("base"),
    )
    .expect("write base");
    fs::write(
        &organization,
        serde_json::to_vec(&layer("organization", "organization_policy", json!({})))
            .expect("organization"),
    )
    .expect("write organization");
    fs::write(
        &personal,
        serde_json::to_vec(&layer("personal", "personal_kit", json!({"theme": "dark"})))
            .expect("personal"),
    )
    .expect("write personal");

    let output = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["compose", "--layer"])
        .arg(&base)
        .arg("--layer")
        .arg(&organization)
        .arg("--layer")
        .arg(&personal)
        .output()
        .expect("compose");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let composition: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(composition["spec"]["theme"], "dark");

    let explanation = Command::new(env!("CARGO_BIN_EXE_commonkit"))
        .args(["explain", "/theme", "--layer"])
        .arg(&base)
        .arg("--layer")
        .arg(&organization)
        .arg("--layer")
        .arg(&personal)
        .output()
        .expect("explain");
    assert!(explanation.status.success());
    let explanation: Value = serde_json::from_slice(&explanation.stdout).expect("JSON");
    assert_eq!(explanation["winner"]["layerId"], "personal");

    fs::remove_dir_all(directory).expect("cleanup");
}
