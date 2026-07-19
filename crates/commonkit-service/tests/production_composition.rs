use commonkit_reconcile::PlanStore;
use commonkit_service::ProductionDomainRegistry;
use serde_json::json;

#[test]
fn production_registry_composes_only_configured_layer_files() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let layer_document = |id: &str, kind: &str, spec: serde_json::Value| {
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
    };
    let layers = [
        ("public-base", "public_base", json!({})),
        ("organization-policy", "organization_policy", json!({})),
        ("personal", "personal_kit", json!({"theme": "dark"})),
    ]
    .map(|(id, kind, spec)| {
        let path = root.join(format!("{id}.json"));
        std::fs::write(
            &path,
            serde_json::to_vec(&layer_document(id, kind, spec)).unwrap(),
        )
        .unwrap();
        path
    });
    let config = root.join("headless.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"composition":{"layers":layers}})).unwrap(),
    )
    .unwrap();
    let registry = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let composition = registry.composition.unwrap();
    assert_eq!(composition.compose().unwrap()["spec"]["theme"], "dark");
    assert_eq!(
        composition.explain("/theme").unwrap()["winner"]["layerId"],
        "personal"
    );
}
