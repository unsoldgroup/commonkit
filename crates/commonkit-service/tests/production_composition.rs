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

#[test]
fn production_registry_rejects_a_later_layer_that_weakens_the_organization_floor() {
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
    let policy = |allowed: &[&str]| {
        json!({
            "deniedPaths": ["**/.env"],
            "requiredControls": {"secret_scan": true},
            "allowlists": {"git_hosts": allowed},
            "minimums": {"backup_count": 1},
            "maximums": {"snapshot_age_hours": 24}
        })
    };
    let layers = [
        ("public-base", "public_base", json!({})),
        (
            "organization-policy",
            "organization_policy",
            json!({"securityPolicy": policy(&["github.com"])}),
        ),
        (
            "personal",
            "personal_kit",
            json!({"securityPolicy": policy(&["github.com", "evil.example"])}),
        ),
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

    let result = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    );

    assert!(result.is_err());
}

#[test]
fn production_registry_uses_schema_merge_rules_and_reports_the_governing_rule() {
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
    let policy = |allowed: &[&str], minimum: i64| {
        json!({
            "deniedPaths": ["**/.env"],
            "requiredControls": {"secret_scan": true},
            "allowlists": {"git_hosts": allowed},
            "minimums": {"backup_count": minimum},
            "maximums": {"snapshot_age_hours": 24}
        })
    };
    let layers = [
        (
            "public-base",
            "public_base",
            json!({"adapters":[{"id":"codex","enabled":true,"settings":{"sandbox":"workspace"}}]}),
        ),
        (
            "organization-policy",
            "organization_policy",
            json!({"securityPolicy": policy(&["github.com", "git.internal"], 1)}),
        ),
        (
            "personal",
            "personal_kit",
            json!({
                "adapters":[{"id":"codex","settings":{"approval":"on-request"}}],
                "securityPolicy": policy(&["github.com"], 3)
            }),
        ),
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

    let composed = composition.compose().unwrap();
    assert_eq!(composed["spec"]["adapters"][0]["enabled"], true);
    assert_eq!(
        composed["spec"]["adapters"][0]["settings"]["approval"],
        "on-request"
    );
    assert_eq!(
        composition.explain("/adapters").unwrap()["governingRules"],
        json!(["schema:/adapters:merge_by_id(id)"])
    );
    assert_eq!(
        composition
            .explain("/securityPolicy/allowlists/git_hosts")
            .unwrap()["governingRules"],
        json!(["organization-security-floor:non-overridable"])
    );
}
