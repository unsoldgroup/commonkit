use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::sync::Arc;

use commonkit_adapters::{
    ArtifactStore, ExactProviderVersion, MaterializedState, ProviderCapability,
    ProviderCapabilityResource, ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{StableId, digest_domain_json};
use commonkit_reconcile::{PlanStore, ReceiptStore};
use commonkit_service::{ApplyStatus, ProductionDomainRegistry};

#[test]
fn local_provider_mcp_clients_share_the_plan_receipt_and_rollback_path() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let target = root.join("target");
    ArtifactStore::open(root.join("artifacts")).unwrap();
    fs::create_dir_all(&target).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("apm").unwrap(),
        ExactProviderVersion::parse("0.25.0").unwrap(),
        "commonkit.apm-provider.v1".into(),
        BTreeMap::from([(
            "manifest".into(),
            digest_domain_json("test.manifest", &"v1").unwrap(),
        )]),
        vec!["agent-context".into()],
    )
    .unwrap();
    let materialized = MaterializedState::finalize_with_capabilities(
        inputs.clone(),
        vec![],
        vec![],
        vec![],
        vec![ProviderCapabilityResource {
            capability: ProviderCapability::McpStreamableHttp {
                id: "docs".into(),
                name: "Docs".into(),
                enabled: true,
                url: "https://upstream.example/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "env:DOCS_TOKEN".into())]),
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id,
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest,
                source: "apm.yml:dependencies.mcp[docs]".into(),
            },
        }],
    )
    .unwrap();
    let state_path = root.join("apm-state.json");
    fs::write(&state_path, serde_json::to_vec(&materialized).unwrap()).unwrap();
    let digest = |value: &str| digest_domain_json("test.relay-client-plan", &value).unwrap();
    let config = root.join("headless.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"sync": {
            "targetId":"local", "targetRoot":target,
            "adapterState":root.join("adapter"), "providerArtifacts":root.join("artifacts"),
            "materializedStates":[state_path], "targetTransport":{"type":"local"},
            "targetPlatform":{"operatingSystem":std::env::consts::OS,"architecture":std::env::consts::ARCH},
            "declaredRoots":["home"], "relayClientRoot":"home",
            "protectedRoots":[], "caseSensitive":true,
            "targetIdentityDigest":digest("target"), "composedLoadoutDigest":digest("loadout"),
            "policyDigest":digest("policy")
        }}))
        .unwrap(),
    )
    .unwrap();
    let plans = Arc::new(PlanStore::open(root.join("plans")).unwrap());
    let receipts_root = root.join("receipts");
    // Model the daemon-owned relay listener. Generated clients use the local
    // authenticated bridge, which discovers this ephemeral address at runtime.
    let relay_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_address = relay_listener.local_addr().unwrap();
    let registry = ProductionDomainRegistry::load_optional_with_relay_endpoint(
        &config,
        plans.clone(),
        receipts_root.clone(),
        relay_address,
    )
    .unwrap();
    let sync = registry.sync.as_ref().unwrap();
    let plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"review"}))
            .unwrap(),
    )
    .unwrap();
    let mut paths = plan
        .operations
        .iter()
        .filter_map(|operation| operation.resource.managed_path.as_deref())
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths, vec!["home/.codex/config.toml", "home/.mcp.json"]);

    let executor = registry
        .target_executors(plans, &receipts_root)
        .unwrap()
        .remove(&StableId::parse("local").unwrap())
        .unwrap();
    assert_eq!(
        executor
            .execute(&plan, &StableId::parse("apply-review").unwrap())
            .status,
        ApplyStatus::Succeeded
    );
    for path in ["home/.mcp.json", "home/.codex/config.toml"] {
        let rendered = fs::read_to_string(target.join(path)).unwrap();
        assert!(rendered.contains("commonkit"));
        assert!(rendered.contains("relay-client"));
        assert!(!rendered.contains(&format!("http://127.0.0.1:{}/mcp", relay_address.port())));
        assert!(!rendered.contains("COMMONKIT_RELAY_TOKEN"));
        assert!(!rendered.contains("upstream.example"));
        assert!(!rendered.contains("DOCS_TOKEN"));
    }
    let receipt_store = ReceiptStore::open(&receipts_root).unwrap();
    let run_id = receipt_store.run_ids().unwrap().pop().unwrap();
    sync.rollback(serde_json::json!({
        "runId":run_id,"planId":plan.id,"confirmed":true,"confirmationId":"rollback-review"
    }))
    .unwrap();
    assert!(!target.join("home/.mcp.json").exists());
    assert!(!target.join("home/.codex/config.toml").exists());
}
