use std::collections::BTreeMap;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, NormalizedResource, ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{StableId, digest_domain_json};
use commonkit_reconcile::PlanStore;
use commonkit_reconcile::{Adapter, ReceiptStore, ReconcileOutcome, Reconciler};
use commonkit_service::ProductionDomainRegistry;

#[cfg(unix)]
#[test]
fn registry_startup_recovers_snapshot_transactions_with_configured_service_lifecycle() {
    use commonkit_snapshots::{
        DatabaseId, DatabaseLifecycle, DurableRestore, InMemoryObjectStore, RestoreFailpoint,
        RestorePlan, SnapshotError, SnapshotService, StaticBackup, XChaCha20Cipher,
        manifest_digest,
    };
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    struct InitialLifecycle;
    impl DatabaseLifecycle for InitialLifecycle {
        fn stop(&mut self) -> Result<(), SnapshotError> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), SnapshotError> {
            Ok(())
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let secret = b"target-local-key";
    let key_path = root.join("key");
    std::fs::write(&key_path, secret).unwrap();
    let key: [u8; 32] = Sha256::digest(secret).into();
    let cipher = XChaCha20Cipher::new(key);
    let mut objects = InMemoryObjectStore::default();
    let database_id = DatabaseId::new("context-mode").unwrap();
    let manifest = SnapshotService::new(&cipher)
        .snapshot(
            &database_id,
            "local",
            None,
            &StaticBackup::new("file", b"restored".to_vec()),
            &mut objects,
        )
        .unwrap();
    let database = root.join("context.db");
    std::fs::write(&database, b"original").unwrap();
    DurableRestore::open(root.join("snapshots/transactions"), &cipher)
        .unwrap()
        .execute(
            RestorePlan {
                schema: "commonkit.restore-plan.v1".into(),
                run_id: "startup-recovery".into(),
                snapshot_id: "snapshot-1".into(),
                manifest_digest: manifest_digest(&manifest).unwrap(),
                database: database_id,
                expected_content_digest: manifest.content_digest.clone(),
            },
            &manifest,
            &objects,
            &database,
            &mut InitialLifecycle,
            RestoreFailpoint::AfterSwap,
        )
        .unwrap_err();

    let lifecycle_log = root.join("lifecycle.log");
    let script = root.join("lifecycle.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
            lifecycle_log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let lifecycle = serde_json::json!({
        "stop": {"executable": script, "args":["stop"]},
        "start": {"executable": script, "args":["start"]}
    });
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":database, "targetId":"local", "format":"file", "lifecycle":lifecycle}]
            }
        }),
    );
    ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    assert_eq!(std::fs::read(root.join("context.db")).unwrap(), b"original");
    assert_eq!(
        std::fs::read_to_string(lifecycle_log).unwrap(),
        "stop\nstart\n"
    );
}

fn write_json(path: &std::path::Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn lifecycle_config() -> serde_json::Value {
    #[cfg(windows)]
    let (executable, args) = (
        std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32/cmd.exe"),
        serde_json::json!(["/C", "exit", "0"]),
    );
    #[cfg(not(windows))]
    let (executable, args) = (
        std::path::PathBuf::from("/usr/bin/true"),
        serde_json::json!([]),
    );
    serde_json::json!({
        "stop": {"executable": executable, "args": args},
        "start": {"executable": executable, "args": args}
    })
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
                content: content.clone(),
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
        "policyDigest": digest_domain_json("test", &"policy").unwrap()
      },
      "credentials": { "root": root.join("credentials"), "destinations": [{
        "id": "api-token", "reference": format!("file://{}", source_secret.display()), "path": "tokens/api"
      }]},
      "snapshots": { "root": root.join("snapshots"), "keyReference": format!("file://{}", source_secret.display()),
        "objectStore": {"type":"local"},
        "databases": [{"id":"context-mode", "path": database, "targetId":"local", "format":"file", "lifecycle": lifecycle_config()}] }
    });
    let config_path = root.join("headless.json");
    write_json(&config_path, &config);
    let plan_store = std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap());
    let registry =
        ProductionDomainRegistry::load(&config_path, plan_store, root.join("receipts")).unwrap();

    let sync = registry.sync.as_ref().unwrap().clone();
    assert_eq!(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test","unexpected":true})),
        Err(commonkit_service::DomainFailure::InvalidRequest)
    );
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

    let stale_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(target.join("home")).unwrap();
    std::fs::write(target.join("home/config.txt"), b"hand edit").unwrap();
    let rebound_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    assert_ne!(stale_plan.observed_digest, rebound_plan.observed_digest);
    let mut stale_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_eq!(
        Reconciler::with_store(&receipts)
            .execute(
                &stale_plan,
                StableId::parse("stale-production-run").unwrap(),
                &mut stale_adapters,
            )
            .unwrap(),
        ReconcileOutcome::Canceled
    );
    std::fs::remove_file(target.join("home/config.txt")).unwrap();

    let artifact_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    let artifact_path = state.join("filesystem/artifacts").join(format!(
        "{}.blob",
        content.digest.as_str().trim_start_matches("sha256:")
    ));
    std::fs::write(&artifact_path, b"tampered").unwrap();
    let mut artifact_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_ne!(
        Reconciler::with_store(&receipts)
            .execute(
                &artifact_plan,
                StableId::parse("tampered-artifact-run").unwrap(),
                &mut artifact_adapters,
            )
            .unwrap(),
        ReconcileOutcome::Succeeded
    );
    assert!(!target.join("home/config.txt").exists());
    std::fs::write(&artifact_path, b"managed\n").unwrap();

    let authenticated_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    let authenticated_run = StableId::parse("authenticated-production-run").unwrap();
    let mut authenticated_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_eq!(
        Reconciler::with_store(&receipts)
            .execute(
                &authenticated_plan,
                authenticated_run.clone(),
                &mut authenticated_adapters,
            )
            .unwrap(),
        ReconcileOutcome::Succeeded
    );
    let receipt_directory = root.join("receipts").join(authenticated_run.as_str());
    let latest_receipt = std::fs::read_dir(receipt_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .max()
        .unwrap();
    std::fs::write(latest_receipt, b"{}").unwrap();
    assert_eq!(
        sync.rollback(
            serde_json::json!({"confirmed":true,"confirmationId":"test","runId":authenticated_run})
        ),
        Err(commonkit_service::DomainFailure::InvalidRequest)
    );

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
    let snapshot_listing = snapshots.list().unwrap().to_string();
    assert!(!snapshot_listing.contains("never serialize me"));
    let encrypted_manifest = std::fs::read(
        std::fs::read_dir(root.join("snapshots/manifests"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(
        !encrypted_manifest
            .windows(b"context-mode".len())
            .any(|window| window == b"context-mode")
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
fn configured_git_provider_pipeline_materializes_native_state_before_local_planning() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let checkout = root.join("checkout");
    git(root, &["init", "--bare", remote.to_str().unwrap()]);
    git(root, &["init", seed.to_str().unwrap()]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "user.name", "CommonKit Test"]);
    std::fs::create_dir_all(seed.join("portable")).unwrap();
    std::fs::write(seed.join("portable/editor.conf"), b"from pinned git\n").unwrap();
    git(&seed, &["add", "portable/editor.conf"]);
    git(&seed, &["commit", "-m", "fixture"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "main"]);
    git(
        root,
        &[
            "clone",
            remote.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git(&checkout, &["checkout", "main"]);
    let revision = git_output(&checkout, &["rev-parse", "HEAD"]);
    let target = root.join("target");
    std::fs::create_dir_all(&target).unwrap();
    let config = serde_json::json!({
      "sync": {
        "targetId": "local", "targetRoot": target,
        "adapterState": root.join("adapter"),
        "providerArtifacts": root.join("artifacts"),
        "providerPipeline": {
          "root": root.join("pipeline"),
          "source": {"repository": checkout, "trustedRemoteUrl": remote.to_str().unwrap(), "revision": revision},
          "providers": [{"provider":"native", "version":"1.0.0", "files":[{
            "path":"home/editor.conf", "source":"portable/editor.conf"
          }]}]
        },
        "declaredRoots": ["home"], "protectedRoots": [], "caseSensitive": true,
        "targetIdentityDigest": digest_domain_json("test", &"target").unwrap(),
        "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
        "policyDigest": digest_domain_json("test", &"policy").unwrap()
      }
    });
    let config_path = root.join("headless-provider.json");
    write_json(&config_path, &config);
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-pipeline")).unwrap()),
        root.join("receipts-pipeline"),
    )
    .unwrap();
    let sync = registry.sync.unwrap();
    let plan = sync
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"provider-plan"}))
        .unwrap();
    assert_eq!(plan["operations"].as_array().unwrap().len(), 1);
    assert_eq!(
        plan["operations"][0]["resource"]["managedPath"],
        "home/editor.conf"
    );
    let states = std::fs::read_dir(root.join("pipeline/states"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(states.len(), 1);
    std::fs::write(
        root.join("checkout/portable/editor.conf"),
        b"uncommitted drift\n",
    )
    .unwrap();
    assert_eq!(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"stale-source"})),
        Err(commonkit_service::DomainFailure::OperationFailed)
    );
}

fn git(directory: &std::path::Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(directory: &std::path::Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
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

#[test]
fn invalid_s3_snapshot_backend_fails_at_registry_load() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let key = root.join("key");
    std::fs::write(&key, b"target-local-key").unwrap();
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "keyReference": format!("file://{}", key.display()),
                "objectStore": {"type":"s3", "executable":"relative/aws", "endpoint":"http://insecure", "bucket":"bad", "prefix":"."},
                "databases": [{"id":"context-mode", "path":root.join("context.sqlite"), "targetId":"local", "format":"sqlite", "lifecycle": lifecycle_config()}]
            }
        }),
    );
    assert!(
        ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
            root.join("receipts"),
        )
        .is_err()
    );
}
