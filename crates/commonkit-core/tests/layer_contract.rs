use commonkit_core::{LayerDocument, LayerKind, PortableSourcePath, Sha256Digest, StableId};

#[test]
fn parses_a_versioned_organization_layer() {
    let layer: LayerDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion": 1,
        "id": "unsold-security",
        "kind": "organization_policy",
        "source": {
            "path": "layers/organization.json",
            "revision": "57a085e",
            "contentDigest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        },
        "spec": {
            "denyPaths": ["**/.env", "**/credentials.json"]
        }
    }))
    .expect("valid layer");

    assert_eq!(layer.schema_version.0, 1);
    assert_eq!(layer.id.as_str(), "unsold-security");
    assert_eq!(layer.kind, LayerKind::OrganizationPolicy);
    assert_eq!(layer.spec["denyPaths"][0], "**/.env");
}

#[test]
fn rejects_unknown_layer_kinds() {
    let error = serde_json::from_value::<LayerDocument>(serde_json::json!({
        "schemaVersion": 1,
        "id": "mystery",
        "kind": "workspace",
        "source": {
            "path": "layers/mystery.json",
            "contentDigest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        },
        "spec": {}
    }))
    .expect_err("unknown kinds fail closed");

    assert!(error.to_string().contains("unknown variant"));
}

#[test]
fn rejects_non_portable_layer_sources_and_invalid_digests() {
    for invalid in [
        "/absolute/layer.json",
        "../outside.json",
        "layers/../outside.json",
        r"layers\windows.json",
        "https://example.com/layer.json",
    ] {
        assert!(PortableSourcePath::parse(invalid).is_err(), "{invalid}");
    }
    assert!(Sha256Digest::parse("sha256:fixture").is_err());
    assert!(Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).is_ok());
}

#[test]
fn rejects_invalid_stable_ids() {
    for invalid in [
        "",
        "Uppercase",
        "two words",
        "starts.with.dot",
        &"a".repeat(64),
    ] {
        assert!(StableId::parse(invalid).is_err(), "{invalid}");
    }
    assert_eq!(
        StableId::parse("target-01").expect("valid").as_str(),
        "target-01"
    );
}
