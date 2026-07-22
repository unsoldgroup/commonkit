#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use commonkit_reconcile::PlanStore;
#[cfg(unix)]
use commonkit_service::ProductionDomainRegistry;
#[cfg(unix)]
use commonkit_service::{CredentialDomain, DomainFailure};

#[cfg(unix)]
fn credential_backup_payload_count(root: &std::path::Path) -> usize {
    fn count(path: &std::path::Path) -> usize {
        match std::fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| {
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        count(&entry.path())
                    } else {
                        1
                    }
                })
                .sum(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("could not inspect credential backups: {error}"),
        }
    }
    count(&root.join("credentials/.commonkit-credentials/backups"))
}

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
    assert_eq!(
        domain.apply(serde_json::json!({
            "destinationIds": ["api-token"],
            "confirmed": true,
            "confirmationId": "credential-unplanned-test"
        })),
        Err(DomainFailure::InvalidRequest)
    );
    let plan = domain
        .plan(serde_json::json!({"destinationIds": ["api-token"]}))
        .unwrap();
    commonkit_contracts::assert_no_embedded_secrets(&plan)
        .expect("credential plans must be safe for the authenticated control API");
    let repeated_plan = domain
        .plan(serde_json::json!({"destinationIds": ["api-token"]}))
        .unwrap();
    assert_eq!(repeated_plan["planId"], plan["planId"]);
    let plan_id = plan["planId"].as_str().unwrap();
    assert!(plan_id.starts_with("sha256:"));
    assert_eq!(plan["operations"][0]["destinationId"], "api-token");
    assert!(!plan.to_string().contains("registry-secret"));
    assert_eq!(
        domain.apply(serde_json::json!({
            "planId": plan_id,
            "confirmed": false,
            "confirmationId": "credential-declined-test"
        })),
        Err(DomainFailure::InvalidRequest)
    );
    drop(domain);
    let registry = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans-after-restart")).unwrap()),
        root.join("receipts-after-restart"),
    )
    .unwrap();
    let domain = registry.credentials.unwrap();
    let result = domain
        .apply(serde_json::json!({
            "planId": plan_id,
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
    let receipts = std::fs::read_dir(root.join("credentials/.commonkit-credentials/receipts"))
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let receipt = String::from_utf8(receipts[0].clone()).unwrap();
    assert!(receipt.contains("succeeded"));
    assert!(!receipt.contains("registry-secret"));
    assert_eq!(credential_backup_payload_count(root), 0);
}

#[cfg(unix)]
#[test]
fn credential_apply_rejects_a_plan_when_the_reviewed_destination_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let runner = root.join("bws-fixture");
    std::fs::write(
        &runner,
        "#!/bin/sh\nprintf '%s' '{\"value\":\"new-secret\"}'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(root.join("credentials/tokens")).unwrap();
    std::fs::write(root.join("credentials/tokens/api"), b"before").unwrap();
    let config_path = root.join("headless.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "credentials": {
                "root": root.join("credentials"),
                "bwsExecutable": runner,
                "destinations": [{"id":"api-token","reference":"bws://secret-id","path":"tokens/api"}]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let domain = registry.credentials.unwrap();
    let plan = domain
        .plan(serde_json::json!({"destinationIds":["api-token"]}))
        .unwrap();
    std::fs::write(root.join("credentials/tokens/api"), b"hand-edit").unwrap();

    let error = domain
        .apply(serde_json::json!({
            "planId": plan["planId"],
            "confirmed": true,
            "confirmationId": "credential-stale-test"
        }))
        .unwrap_err();

    assert_eq!(error, DomainFailure::StalePlan);
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/api")).unwrap(),
        b"hand-edit"
    );
}

#[cfg(unix)]
#[test]
fn credential_apply_rolls_back_all_destinations_and_writes_a_redacted_receipt_on_failure() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let runner = root.join("bws-fixture");
    std::fs::write(
        &runner,
        "#!/bin/sh\nprintf '{\"value\":\"resolved-%s\"}' \"$3\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(root.join("credentials/tokens/broken")).unwrap();
    std::fs::write(root.join("credentials/tokens/first"), b"original-first").unwrap();
    let config_path = root.join("headless.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "credentials": {
                "root": root.join("credentials"),
                "bwsExecutable": runner,
                "destinations": [
                    {"id":"first","reference":"bws://one","path":"tokens/first"},
                    {"id":"second","reference":"bws://two","path":"tokens/broken"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let domain = registry.credentials.unwrap();
    let plan = domain
        .plan(serde_json::json!({"destinationIds":["first", "second"]}))
        .unwrap();

    assert_eq!(
        domain.apply(serde_json::json!({
            "planId": plan["planId"],
            "confirmed": true,
            "confirmationId": "credential-atomic-test"
        })),
        Err(DomainFailure::OperationFailed)
    );
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/first")).unwrap(),
        b"original-first"
    );
    let receipts = std::fs::read_dir(root.join("credentials/.commonkit-credentials/receipts"))
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let receipt = String::from_utf8(receipts[0].clone()).unwrap();
    assert!(receipt.contains("rolledBack"));
    assert!(!receipt.contains("resolved-one"));
    assert!(!receipt.contains("original-first"));
    assert_eq!(credential_backup_payload_count(root), 0);
}

#[cfg(unix)]
#[test]
fn credential_apply_restores_an_empty_regular_file_when_a_later_destination_fails() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let runner = root.join("bws-fixture");
    std::fs::write(
        &runner,
        "#!/bin/sh\nprintf '{\"value\":\"resolved-%s\"}' \"$3\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(root.join("credentials/tokens/broken")).unwrap();
    std::fs::write(root.join("credentials/tokens/empty"), b"").unwrap();
    let config_path = root.join("headless.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "credentials": {
                "root": root.join("credentials"),
                "bwsExecutable": runner,
                "destinations": [
                    {"id":"empty","reference":"bws://one","path":"tokens/empty"},
                    {"id":"broken","reference":"bws://two","path":"tokens/broken"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let domain = registry.credentials.unwrap();
    let plan = domain
        .plan(serde_json::json!({"destinationIds":["empty", "broken"]}))
        .unwrap();

    assert_eq!(
        domain.apply(serde_json::json!({
            "planId": plan["planId"],
            "confirmed": true,
            "confirmationId": "credential-empty-backup-test"
        })),
        Err(DomainFailure::OperationFailed)
    );
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/empty")).unwrap(),
        b""
    );
}

#[cfg(unix)]
#[test]
fn credential_configuration_rejects_duplicate_and_parent_child_destinations() {
    for (name, paths) in [
        ("duplicate", ("tokens/api", "tokens/api")),
        ("overlap", ("tokens", "tokens/api")),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let config_path = root.join(format!("{name}.json"));
        std::fs::write(
            &config_path,
            serde_json::to_vec(&serde_json::json!({
                "credentials": {
                    "root": root.join("credentials"),
                    "destinations": [
                        {"id":"first","reference":"env://FIRST","path":paths.0},
                        {"id":"second","reference":"env://SECOND","path":paths.1}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let result = ProductionDomainRegistry::load(
            &config_path,
            Arc::new(PlanStore::open(root.join("plans")).unwrap()),
            root.join("receipts"),
        );

        assert!(result.is_err(), "{name} destinations must fail closed");
    }
}

#[cfg(unix)]
#[test]
fn registry_startup_recovers_an_interrupted_multi_destination_credential_apply() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let (config_path, domain) = leave_interrupted_credential_apply(root, b"");
    drop(domain);

    let restarted = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans-restarted")).unwrap()),
        root.join("receipts-restarted"),
    );

    assert!(restarted.is_ok());
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/first")).unwrap(),
        b""
    );
    assert!(root.join("credentials/tokens/second").is_dir());
    let receipt_path = std::fs::read_dir(root.join("credentials/.commonkit-credentials/receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let recovered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt_path).unwrap()).unwrap();
    assert_eq!(recovered["status"], "rolledBack");
    let durable = recovered.to_string();
    assert!(!durable.contains("original-first"));
    assert!(!durable.contains("new-one"));
    assert_eq!(credential_backup_payload_count(root), 0);
}

#[cfg(unix)]
#[test]
fn corrupt_credential_recovery_artifact_blocks_apply_and_restart_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let (config_path, domain) = leave_interrupted_credential_apply(root, b"original-first");
    let backup_run = std::fs::read_dir(root.join("credentials/.commonkit-credentials/backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let backup_path = std::fs::read_dir(backup_run)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(backup_path, b"tampered-backup").unwrap();

    assert_eq!(
        domain.apply(serde_json::json!({
            "planId":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "confirmed":true,
            "confirmationId":"must-not-apply"
        })),
        Err(DomainFailure::OperationFailed)
    );
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/first")).unwrap(),
        b"new-one"
    );
    drop(domain);
    assert!(
        ProductionDomainRegistry::load(
            &config_path,
            Arc::new(PlanStore::open(root.join("plans-restarted")).unwrap()),
            root.join("receipts-restarted"),
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/first")).unwrap(),
        b"new-one"
    );
}

#[cfg(unix)]
#[test]
fn missing_credential_recovery_artifact_blocks_restart_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let (config_path, domain) = leave_interrupted_credential_apply(root, b"original-first");
    let backup_run = std::fs::read_dir(root.join("credentials/.commonkit-credentials/backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let backup_path = std::fs::read_dir(backup_run)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(backup_path).unwrap();
    drop(domain);

    assert!(
        ProductionDomainRegistry::load(
            &config_path,
            Arc::new(PlanStore::open(root.join("plans-restarted")).unwrap()),
            root.join("receipts-restarted"),
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/first")).unwrap(),
        b"new-one"
    );
}

#[cfg(unix)]
#[test]
fn credential_apply_crash_child() {
    let Some(config_path) = std::env::var_os("COMMONKIT_CREDENTIAL_CRASH_CONFIG") else {
        return;
    };
    let plan_id = std::env::var("COMMONKIT_CREDENTIAL_CRASH_PLAN").unwrap();
    let state =
        std::path::PathBuf::from(std::env::var_os("COMMONKIT_CREDENTIAL_CRASH_STATE").unwrap());
    let registry = ProductionDomainRegistry::load(
        std::path::Path::new(&config_path),
        Arc::new(PlanStore::open(state.join("child-plans")).unwrap()),
        state.join("child-receipts"),
    )
    .unwrap();
    let _ = registry.credentials.unwrap().apply(serde_json::json!({
        "planId":plan_id,
        "confirmed":true,
        "confirmationId":"crash-recovery"
    }));
}

#[cfg(unix)]
fn leave_interrupted_credential_apply(
    root: &std::path::Path,
    original_first: &[u8],
) -> (std::path::PathBuf, Arc<dyn CredentialDomain>) {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let runner = root.join("bws-fixture");
    std::fs::write(
        &runner,
        "#!/bin/sh\nprintf '{\"value\":\"new-%s\"}' \"$3\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(root.join("credentials/tokens")).unwrap();
    std::fs::write(root.join("credentials/tokens/first"), original_first).unwrap();
    std::fs::create_dir(root.join("credentials/tokens/second")).unwrap();
    let config_path = root.join("headless.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "credentials": {
                "root": root.join("credentials"), "bwsExecutable": runner,
                "destinations": [
                    {"id":"first","reference":"bws://one","path":"tokens/first"},
                    {"id":"second","reference":"bws://two","path":"tokens/second"}
                ]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = ProductionDomainRegistry::load(
        &config_path,
        Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let domain = registry.credentials.unwrap();
    let plan = domain
        .plan(serde_json::json!({"destinationIds":["first","second"]}))
        .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "credential_apply_crash_child", "--nocapture"])
        .env("COMMONKIT_CREDENTIAL_CRASH_CONFIG", &config_path)
        .env(
            "COMMONKIT_CREDENTIAL_CRASH_PLAN",
            plan["planId"].as_str().unwrap(),
        )
        .env("COMMONKIT_CREDENTIAL_CRASH_STATE", root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read(root.join("credentials/tokens/first")).unwrap() != b"new-one" {
        assert!(
            Instant::now() < deadline,
            "child did not reach the second destination"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before interruption"
        );
        std::hint::spin_loop();
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(credential_backup_payload_count(root), 1);
    let backup_root = root.join("credentials/.commonkit-credentials/backups");
    let backup_run = std::fs::read_dir(&backup_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let backup_file = std::fs::read_dir(&backup_run)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::metadata(&backup_run).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(backup_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let receipt = std::fs::read_to_string(
        std::fs::read_dir(root.join("credentials/.commonkit-credentials/receipts"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(!receipt.contains("original-first"));
    assert!(!receipt.contains("new-one"));
    (config_path, domain)
}
