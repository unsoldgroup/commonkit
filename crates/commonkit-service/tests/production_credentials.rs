#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use commonkit_reconcile::PlanStore;
#[cfg(unix)]
use commonkit_service::ProductionDomainRegistry;

#[cfg(unix)]
#[test]
fn production_registry_resolves_bws_only_during_confirmed_apply_without_leaking_value() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let runner = root.join("bws-fixture");
    std::fs::write(
        &runner,
        "#!/bin/sh\n[ \"$1\" = secret ] && [ \"$2\" = get ] && [ \"$3\" = secret-id ] || exit 7\nprintf '%s' '{\"value\":\"registry-secret\"}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = serde_json::json!({
        "credentials": {
            "root": root.join("credentials"),
            "bwsExecutable": runner,
            "destinations": [{
                "id": "api-token",
                "reference": "bws://secret-id",
                "path": "tokens/api"
            }]
        }
    });
    let config_path = root.join("headless.json");
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let registry = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let domain = registry.credentials.unwrap();
    let result = domain
        .apply(serde_json::json!({
            "destinationIds": ["api-token"],
            "confirmed": true,
            "confirmationId": "credential-test"
        }))
        .unwrap();

    assert_eq!(result["applied"][0], "api-token");
    assert!(!result.to_string().contains("registry-secret"));
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/api")).unwrap(),
        b"registry-secret"
    );
}
