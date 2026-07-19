#![cfg(unix)]

use std::path::Path;

use commonkit_cli::onboarding::{InitMode, InitRequest, ProcessRunner, initialize};

fn request(root: &Path, mode: InitMode, repository: &str) -> InitRequest {
    InitRequest {
        mode,
        repository: repository.to_owned(),
        kit_directory: root.join("kit"),
        loadout: "personal".to_owned(),
        target: "workstation".to_owned(),
        target_root: root.join("target"),
        config_directory: root.join("config"),
        state_directory: root.join("state"),
    }
}

#[test]
fn connect_uses_existing_gh_credentials_and_writes_first_run_state() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    test_support::write_tool(
        &bin,
        "gh",
        r#"
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers"
    printf '{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/public-base.json"
    printf '{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/organization-policy.json"
    printf '{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/personal.json"
    exit 0 ;;
esac
exit 91
"#,
    );
    test_support::write_tool(
        &bin,
        "git",
        r#"
case "$3" in
  rev-parse) printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'; exit 0 ;;
esac
exit 92
"#,
    );
    let result = initialize(
        &request(
            temporary.path(),
            InitMode::Connect,
            "unsoldgroup/commonkit-config",
        ),
        &ProcessRunner::new(&bin),
    )
    .unwrap();

    assert_eq!(result.repository, "unsoldgroup/commonkit-config");
    assert!(result.headless_config.ends_with("headless.json"));
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result.headless_config).unwrap()).unwrap();
    assert_eq!(config["composition"]["layers"].as_array().unwrap().len(), 3);
    assert_eq!(config["sync"]["targetId"], "workstation");
    assert_eq!(
        config["sync"]["materializedStates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let serialized = serde_json::to_string(&config).unwrap();
    assert!(!serialized.to_ascii_lowercase().contains("token"));
    assert!(!serialized.contains("ghp_"));

    let plans = std::sync::Arc::new(
        commonkit_reconcile::PlanStore::open(temporary.path().join("plans")).unwrap(),
    );
    let registry = commonkit_service::ProductionDomainRegistry::load(
        &result.headless_config,
        plans,
        temporary.path().join("receipts"),
    )
    .unwrap();
    let composition = registry.composition.unwrap().compose().unwrap();
    assert_eq!(composition["spec"], serde_json::json!({}));
    let plan = registry
        .sync
        .unwrap()
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"first-diff"}))
        .unwrap();
    assert_eq!(plan["operations"], serde_json::json!([]));
}

#[test]
fn create_provisions_a_private_repository_and_pushes_only_portable_files() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands.log");
    std::fs::create_dir_all(&bin).unwrap();
    test_support::write_logging_tools(&bin, &log);

    let result = initialize(
        &request(temporary.path(), InitMode::Create, "owner/new-kit"),
        &ProcessRunner::new(&bin),
    )
    .unwrap();
    assert!(result.kit_directory.join("layers/personal.json").is_file());
    assert!(
        result
            .kit_directory
            .join("targets/workstation.json")
            .is_file()
    );
    let commands = std::fs::read_to_string(log).unwrap();
    assert!(commands.contains("gh repo create owner/new-kit --private"));
    assert!(commands.contains("gh repo clone owner/new-kit"));
    assert!(commands.contains("git -C"));
    assert!(commands.contains("push --set-upstream origin HEAD"));
    assert!(!commands.to_ascii_lowercase().contains("token"));
}

#[test]
fn rejects_repository_flags_paths_and_incomplete_existing_destinations() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = ProcessRunner::new(temporary.path());
    for repository in [
        "--help",
        "owner",
        "owner/repo/extra",
        "https://github.com/owner/repo",
    ] {
        let error = initialize(
            &request(temporary.path(), InitMode::Connect, repository),
            &runner,
        )
        .unwrap_err();
        assert!(error.to_string().contains("owner/repository"));
    }
}

mod test_support {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    pub fn write_tool(bin: &Path, name: &str, body: &str) {
        let path = bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    pub fn write_logging_tools(bin: &Path, log: &Path) {
        let quoted = format!("'{}'", log.display());
        write_tool(
            bin,
            "gh",
            &format!(
                r#"
printf 'gh %s\n' "$*" >> {quoted}
if [ "$1 $2" = "auth status" ]; then exit 0; fi
if [ "$1 $2" = "repo create" ]; then exit 0; fi
if [ "$1 $2" = "repo clone" ]; then mkdir -p "$4"; exit 0; fi
exit 91
"#
            ),
        );
        write_tool(
            bin,
            "git",
            &format!(
                r#"
printf 'git %s\n' "$*" >> {quoted}
if [ "$3" = "rev-parse" ]; then printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'; fi
exit 0
"#
            ),
        );
    }
}
