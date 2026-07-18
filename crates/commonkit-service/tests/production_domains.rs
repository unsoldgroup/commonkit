use std::collections::BTreeMap;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, NormalizedResource, ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{StableId, digest_domain_json};
use commonkit_reconcile::PlanStore;
use commonkit_reconcile::{Adapter, ReceiptStore, ReconcileOutcome, Reconciler};
use commonkit_service::ProductionDomainRegistry;

fn write_json(path: &std::path::Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn configured_registry_materializes_real_plans_credentials_and_snapshots() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let target = root.join("target");
    let state = root.join("state");
    let provider_artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let content = provider_artifacts
        .put(b"managed\n", ContentSensitivity::Portable)
        .unwrap();
    let input_digest = digest_domain_json("test.input", &"native").unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "1".into(),
        BTreeMap::from([("manifest".into(), input_digest)]),
        vec!["files".into()],
    )
    .unwrap();
    let materialized = MaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/config.txt").unwrap(),
                content,
                mode: None,
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "fixture".into(),
            },
        }],
        vec![],
        vec![],
    )
    .unwrap();
    let materialized_path = root.join("materialized.json");
    write_json(&materialized_path, &materialized);
    let source_secret = root.join("source.secret");
    std::fs::write(&source_secret, b"never serialize me").unwrap();
    let database = root.join("context.sqlite");
    std::fs::write(&database, b"database-v1").unwrap();
    let config = serde_json::json!({
      "sync": {
        "targetId": "local", "targetRoot": target, "adapterState": state.join("filesystem"),
        "providerArtifacts": root.join("provider-artifacts"), "materializedStates": [materialized_path],
        "declaredRoots": ["home"], "protectedRoots": [], "caseSensitive": true,
        "targetIdentityDigest": digest_domain_json("test", &"target").unwrap(),
        "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
        "observedDigest": digest_domain_json("test", &"observed").unwrap(),
        "policyDigest": digest_domain_json("test", &"policy").unwrap()
      },
      "credentials": { "root": root.join("credentials"), "destinations": [{
        "id": "api-token", "reference": format!("file://{}", source_secret.display()), "path": "tokens/api"
      }]},
      "snapshots": { "root": root.join("snapshots"), "keyReference": format!("file://{}", source_secret.display()),
        "databases": [{"id":"context-mode", "path": database, "targetId":"local", "format":"file"}] }
    });
    let config_path = root.join("headless.json");
    write_json(&config_path, &config);
    let plan_store = std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap());
    let registry =
        ProductionDomainRegistry::load(&config_path, plan_store, root.join("receipts")).unwrap();

    let sync = registry.sync.as_ref().unwrap().clone();
    let plan = sync
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
        .unwrap();
    assert_eq!(plan["targetId"], "local");
    assert_eq!(
        plan["operations"][0]["resource"]["managedPath"],
        "home/config.txt"
    );
    let plan_contract: commonkit_contracts::Plan = serde_json::from_value(plan).unwrap();
    let receipts = ReceiptStore::open(root.join("receipts")).unwrap();
    let run_id = StableId::parse("production-run").unwrap();
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_eq!(
        Reconciler::with_store(&receipts)
            .execute(&plan_contract, run_id.clone(), &mut adapters)
            .unwrap(),
        ReconcileOutcome::Succeeded
    );
    assert_eq!(
        std::fs::read(target.join("home/config.txt")).unwrap(),
        b"managed\n"
    );
    let rolled_back = sync
        .rollback(serde_json::json!({"confirmed":true,"confirmationId":"test","runId":run_id}))
        .unwrap();
    assert_eq!(rolled_back["outcome"], "rolledback");
    assert!(!target.join("home/config.txt").exists());

    let credentials = registry.credentials.unwrap();
    let applied = credentials.apply(serde_json::json!({"confirmed":true,"confirmationId":"test","destinationIds":["api-token"]})).unwrap();
    assert_eq!(applied, serde_json::json!({"applied":["api-token"]}));
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/api")).unwrap(),
        b"never serialize me"
    );
    assert!(!applied.to_string().contains("never serialize me"));

    let snapshots = registry.snapshots.unwrap();
    let created = snapshots.create(serde_json::json!({"confirmed":true,"confirmationId":"test","databaseId":"context-mode"})).unwrap();
    assert_eq!(created["databaseId"], "context-mode");
    assert_eq!(
        snapshots.list().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    std::fs::write(&database, b"database-v2").unwrap();
    snapshots.restore(serde_json::json!({"confirmed":true,"confirmationId":"test","snapshotId":created["snapshotId"]})).unwrap();
    assert_eq!(std::fs::read(database).unwrap(), b"database-v1");
    snapshots.promote(serde_json::json!({"confirmed":true,"confirmationId":"test","databaseId":"context-mode","targetId":"workstation-b"})).unwrap();
    assert_eq!(
        snapshots.list().unwrap()["writers"]["context-mode"],
        "workstation-b"
    );
}

#[test]
fn absent_configuration_installs_no_fabricated_domains() {
    let temporary = tempfile::tempdir().unwrap();
    let registry = ProductionDomainRegistry::load_optional(
        &temporary.path().join("missing.json"),
        std::sync::Arc::new(PlanStore::open(temporary.path().join("plans")).unwrap()),
        temporary.path().join("receipts"),
    )
    .unwrap();
    assert!(registry.sync.is_none());
    assert!(registry.credentials.is_none());
    assert!(registry.snapshots.is_none());
}

#[test]
fn present_but_incomplete_capability_configuration_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": {"root":"relative", "destinations":[]},
            "snapshots": null
        }),
    );
    assert!(
        ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(temporary.path().join("plans")).unwrap()),
            temporary.path().join("receipts"),
        )
        .is_err()
    );
}
