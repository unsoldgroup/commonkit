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
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":database, "targetId":"local", "observedPaths":{"workstation-b":root.join("context-candidate.db")}, "format":"file", "lifecycle":lifecycle}]
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
      "snapshots": { "root": root.join("snapshots"), "portableState": root.join("kit"), "keyReference": format!("file://{}", source_secret.display()),
        "objectStore": {"type":"local"},
        "databases": [{"id":"context-mode", "path": database, "targetId":"local", "observedPaths":{"workstation-b":root.join("context-candidate.db")}, "format":"file", "lifecycle": lifecycle_config()}] }
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
    let encrypted_objects = std::fs::read_dir(root.join("snapshots/objects"))
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(encrypted_objects.len(), 2);
    assert!(encrypted_objects.iter().all(|object| {
        !object
            .windows(b"context-mode".len())
            .any(|window| window == b"context-mode")
            && !object
                .windows(b"never serialize me".len())
                .any(|window| window == b"never serialize me")
    }));
    std::fs::write(&database, b"database-v2").unwrap();
    snapshots.restore(serde_json::json!({"confirmed":true,"confirmationId":"test","snapshotId":created["snapshotId"]})).unwrap();
    assert_eq!(std::fs::read(database).unwrap(), b"database-v1");
    std::fs::write(root.join("context-candidate.db"), b"database-v1").unwrap();
    snapshots.promote(serde_json::json!({"confirmed":true,"confirmationId":"test","databaseId":"context-mode","targetId":"workstation-b"})).unwrap();
    assert_eq!(
        snapshots.list().unwrap()["writers"]["context-mode"],
        "workstation-b"
    );
}

#[test]
fn snapshot_promotion_rejects_unsnapshotted_authoritative_changes_and_missing_observations() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let database = root.join("writer.db");
    let candidate = root.join("candidate.db");
    std::fs::write(&database, b"snapshot-state").unwrap();
    std::fs::write(&candidate, b"snapshot-state").unwrap();
    let key_path = root.join("snapshot-key");
    std::fs::write(&key_path, b"snapshot-test-key").unwrap();
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "databases": [{
                    "id":"context-mode",
                    "path": database,
                    "targetId":"writer",
                    "observedPaths":{"candidate":candidate},
                    "format":"file",
                    "lifecycle":lifecycle_config()
                }]
            }
        }),
    );
    let registry = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let snapshots = registry.snapshots.unwrap();
    assert_eq!(
        snapshots.list().unwrap()["promotionEvidence"]["context-mode"]["observableTargets"],
        serde_json::json!(["candidate", "writer"])
    );
    snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    std::fs::write(&database, b"newer-authoritative-state").unwrap();

    assert_eq!(
        snapshots.promote(serde_json::json!({"databaseId":"context-mode","targetId":"candidate"})),
        Err(commonkit_service::DomainFailure::VerificationFailed)
    );
    assert_eq!(
        snapshots.list().unwrap()["writers"]["context-mode"],
        "writer"
    );
    assert_eq!(
        snapshots.promote(serde_json::json!({
            "databaseId":"context-mode",
            "targetId":"unconfigured",
            "currentWriterDigest":"sha256:caller-assertion",
            "candidateDigest":"sha256:caller-assertion"
        })),
        Err(commonkit_service::DomainFailure::InvalidRequest)
    );
}

#[test]
fn promoted_writer_is_the_only_source_allowed_to_advance_snapshot_history() {
    use sha2::{Digest, Sha256};

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let writer_a = root.join("writer-a.db");
    let writer_b = root.join("writer-b.db");
    std::fs::write(&writer_a, b"state-v1").unwrap();
    std::fs::write(&writer_b, b"state-v1").unwrap();
    let key_path = root.join("snapshot-key");
    std::fs::write(&key_path, b"snapshot-test-key").unwrap();
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "databases": [{
                    "id":"context-mode",
                    "path":writer_a,
                    "targetId":"machine-a",
                    "observedPaths":{"machine-b":writer_b},
                    "format":"file",
                    "lifecycle":lifecycle_config()
                }]
            }
        }),
    );
    let snapshots = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap()
    .snapshots
    .unwrap();

    let first = snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    snapshots
        .promote(serde_json::json!({
            "databaseId":"context-mode",
            "targetId":"machine-b",
            "confirmationId":"promote-to-b"
        }))
        .unwrap();
    std::fs::write(&writer_b, b"state-v2-from-b").unwrap();
    let second = snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();

    let listing = snapshots.list().unwrap();
    let manifests = listing["snapshots"].as_array().unwrap();
    let second_manifest = manifests
        .iter()
        .find(|item| item["snapshotId"] == second["snapshotId"])
        .unwrap();
    assert_eq!(second_manifest["manifest"]["sourceTarget"], "machine-b");
    assert_eq!(
        second_manifest["manifest"]["parentDigest"],
        format!("sha256:{:x}", Sha256::digest(b"state-v1"))
    );

    // A stale former writer cannot create a new branch or become authoritative again merely by
    // changing its local bytes. With B unchanged, create is a no-op at the true chain head.
    std::fs::write(&writer_a, b"state-v3-from-old-a").unwrap();
    let after_old_writer_change = snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    assert_eq!(after_old_writer_change["snapshotId"], second["snapshotId"]);
    assert_ne!(first["snapshotId"], second["snapshotId"]);
    assert_eq!(
        snapshots.list().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn portable_descriptor_discovers_and_restores_snapshot_on_another_machine() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let portable_state = root.join("kit");
    let key = root.join("provisioned-snapshot-key");
    std::fs::write(&key, b"same-key-provisioned-outside-git").unwrap();
    let source_database = root.join("machine-a.db");
    std::fs::write(&source_database, b"portable database plaintext").unwrap();

    let config_a = root.join("machine-a.json");
    write_json(
        &config_a,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("machine-a-state"),
                "portableState": portable_state,
                "keyReference": format!("file://{}", key.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":source_database, "targetId":"machine-a", "format":"file", "lifecycle":lifecycle_config()}]
            }
        }),
    );
    let snapshots_a = ProductionDomainRegistry::load(
        &config_a,
        std::sync::Arc::new(PlanStore::open(root.join("plans-a")).unwrap()),
        root.join("receipts-a"),
    )
    .unwrap()
    .snapshots
    .unwrap();
    let created = snapshots_a
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    drop(snapshots_a);

    // A shared S3 backend supplies these immutable ciphertext objects in production. Copying the
    // local test backend models the same object availability without putting database bytes in Git.
    let machine_b_state = root.join("machine-b-state");
    std::fs::create_dir_all(machine_b_state.join("objects")).unwrap();
    for entry in std::fs::read_dir(root.join("machine-a-state/objects")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            machine_b_state.join("objects").join(entry.file_name()),
        )
        .unwrap();
    }
    let candidate_database = root.join("machine-b.db");
    std::fs::write(&candidate_database, b"stale local database").unwrap();
    let config_b = root.join("machine-b.json");
    write_json(
        &config_b,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": machine_b_state,
                "portableState": portable_state,
                "keyReference": format!("file://{}", key.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":candidate_database, "targetId":"machine-b", "format":"file", "lifecycle":lifecycle_config()}]
            }
        }),
    );
    let snapshots_b = ProductionDomainRegistry::load(
        &config_b,
        std::sync::Arc::new(PlanStore::open(root.join("plans-b")).unwrap()),
        root.join("receipts-b"),
    )
    .unwrap()
    .snapshots
    .unwrap();
    assert_eq!(
        snapshots_b.list().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    snapshots_b
        .restore(serde_json::json!({"snapshotId":created["snapshotId"]}))
        .unwrap();
    assert_eq!(
        std::fs::read(candidate_database).unwrap(),
        b"portable database plaintext"
    );
    for entry in std::fs::read_dir(root.join("kit/snapshots")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(b"portable database plaintext".len())
                .any(|window| { window == b"portable database plaintext" })
        );
        assert!(
            !bytes
                .windows(b"same-key-provisioned-outside-git".len())
                .any(|window| { window == b"same-key-provisioned-outside-git" })
        );
    }
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
                "portableState": root.join("kit"),
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
