#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use commonkit_cli::onboarding::{
    CommandRunner, InitMode, InitRequest, OnboardingError, ProcessRunner, ProviderSelection,
    initialize, register_agent_plugin_marketplace,
};
use commonkit_contracts::LayerDocument;
use std::ffi::OsString;
use std::process::Command;

#[test]
fn plugin_marketplace_registration_installs_claude_and_codex_hooks() {
    let temporary = tempfile::tempdir().unwrap();
    let kit = temporary.path().join("kit");
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands.log");
    std::fs::create_dir_all(kit.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(kit.join("plugin/hooks")).unwrap();
    std::fs::create_dir_all(kit.join("plugin/.codex-plugin")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        kit.join(".claude-plugin/marketplace.json"),
        r#"{"name":"commonkit","plugins":[{"name":"commonkit-kit","source":"./plugin"}]}"#,
    )
    .unwrap();
    std::fs::write(kit.join("plugin/hooks/hooks.json"), "{}").unwrap();
    std::fs::write(kit.join("plugin/.codex-plugin/hooks.json"), "{}").unwrap();
    for tool in ["claude", "codex"] {
        test_support::write_tool(
            &bin,
            tool,
            &format!(
                "printf '%s %s\\n' '{}' \"$*\" >> '{}'\n",
                tool,
                log.display()
            ),
        );
    }

    let installed = register_agent_plugin_marketplace(
        &kit,
        "example-user/commonkit",
        &ProcessRunner::new(&bin),
    )
    .unwrap();

    assert_eq!(installed, vec!["claude", "codex"]);
    let commands = std::fs::read_to_string(log).unwrap();
    assert!(commands.contains("claude plugin marketplace add example-user/commonkit"));
    assert!(commands.contains("claude plugin install commonkit-kit@commonkit"));
    assert!(commands.contains("codex plugin marketplace add example-user/commonkit"));
    assert!(commands.contains("codex plugin add commonkit-kit@commonkit"));
}

fn request(root: &Path, mode: InitMode, repository: &str) -> InitRequest {
    InitRequest {
        mode,
        repository: repository.to_owned(),
        kit_directory: root.join("kit"),
        loadout: "personal".to_owned(),
        project_loadout: None,
        target_override: None,
        target: "workstation".to_owned(),
        target_root: root.join("target"),
        config_directory: root.join("config"),
        state_directory: root.join("state"),
        provider: ProviderSelection::Native,
        publish_registration: true,
    }
}

#[test]
fn connect_configures_all_five_layers_with_provenance_and_an_organization_floor() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("target")).unwrap();
    test_support::write_tool(
        &bin,
        "gh",
        r#"
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers"
    printf '%s' '{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"contextBudget":{"maxTotalTokens":100}}}' > "$4/layers/public-base.json"
    printf '%s' '{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"contextBudget":{"maxTotalTokens":95},"securityPolicy":{"deniedPaths":["**/.env"],"requiredControls":{"secret_scan":true},"allowlists":{"git_hosts":["github.com"]},"minimums":{"backup_count":1},"maximums":{"snapshot_age_hours":24}}}}' > "$4/layers/organization-policy.json"
    printf '%s' '{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"contextBudget":{"maxTotalTokens":90}}}' > "$4/layers/personal.json"
    printf '%s' '{"schemaVersion":1,"id":"project-web","kind":"project_loadout","source":{"path":"layers/project-web.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"contextBudget":{"maxTotalTokens":85}}}' > "$4/layers/project-web.json"
    printf '%s' '{"schemaVersion":1,"id":"target-workstation","kind":"target_overrides","source":{"path":"layers/target-workstation.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"contextBudget":{"maxTotalTokens":80}}}' > "$4/layers/target-workstation.json"
    exit 0 ;;
esac
exit 91
"#,
    );
    test_support::write_tool(
        &bin,
        "git",
        "[ \"$3\" = rev-parse ] && printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\nexit 0",
    );
    let mut request = request(temporary.path(), InitMode::Connect, "owner/five-layer-kit");
    request.project_loadout = Some("project-web".to_owned());
    request.target_override = Some("target-workstation".to_owned());

    let result = initialize(&request, &ProcessRunner::new(&bin)).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result.headless_config).unwrap()).unwrap();
    assert_eq!(config["composition"]["layers"].as_array().unwrap().len(), 5);

    let plans = std::sync::Arc::new(
        commonkit_reconcile::PlanStore::open(temporary.path().join("state/plans")).unwrap(),
    );
    let registry = commonkit_service::ProductionDomainRegistry::load(
        &result.headless_config,
        plans,
        temporary.path().join("receipts"),
    )
    .unwrap();
    let composition = registry.composition.unwrap();
    let composed = composition.compose().unwrap();
    assert_eq!(composed["spec"]["contextBudget"]["maxTotalTokens"], 80);
    for (index, path) in config["composition"]["layers"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let document: LayerDocument =
            serde_json::from_slice(&std::fs::read(path.as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(
            composed["lock"]["layers"][index]["contentDigest"],
            serde_json::json!(
                commonkit_config::layer_content_digest(&document)
                    .unwrap()
                    .to_string()
            )
        );
    }
    let budget = composition
        .explain("/contextBudget/maxTotalTokens")
        .unwrap();
    assert_eq!(budget["winner"]["layerId"], "target-workstation");
    assert_eq!(budget["contributions"].as_array().unwrap().len(), 5);
    assert_eq!(
        composition.explain("/securityPolicy/deniedPaths").unwrap()["governingRules"],
        serde_json::json!([
            "schema:/securityPolicy/deniedPaths:set_union",
            "organization-security-floor:non-overridable"
        ])
    );
}

#[test]
fn connect_rejects_a_layer_with_mismatched_canonical_content_before_registration() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("target")).unwrap();
    test_support::write_raw_tool(
        &bin,
        "gh",
        r#"
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers"
    printf '%s' '{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/public-base.json"
    printf '%s' '{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/organization-policy.json"
    printf '%s' '{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"tampered":true}}' > "$4/layers/personal.json"
    exit 0 ;;
esac
exit 91
"#,
    );
    test_support::write_tool(
        &bin,
        "git",
        "[ \"$3\" = rev-parse ] && printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\nexit 0",
    );
    let request = request(temporary.path(), InitMode::Connect, "owner/tampered-kit");

    let error = initialize(&request, &ProcessRunner::new(&bin)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("layer content digest does not match canonical content")
    );
    assert!(
        !request
            .kit_directory
            .join("targets/workstation.json")
            .exists()
    );
    assert!(!request.config_directory.join("headless.json").exists());
}

#[test]
fn connect_rejects_a_weakened_organization_floor_before_registration_or_plan_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands.log");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("target")).unwrap();
    test_support::write_tool(
        &bin,
        "gh",
        &format!(
            r#"
printf 'gh %s\n' "$*" >> '{}'
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers"
    printf '%s' '{{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/public-base.json"
    printf '%s' '{{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{"securityPolicy":{{"deniedPaths":["**/.env"],"requiredControls":{{"secret_scan":true}},"allowlists":{{"git_hosts":["github.com"]}},"minimums":{{"backup_count":1}},"maximums":{{"snapshot_age_hours":24}}}}}}}}' > "$4/layers/organization-policy.json"
    printf '%s' '{{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/personal.json"
    printf '%s' '{{"schemaVersion":1,"id":"project-web","kind":"project_loadout","source":{{"path":"layers/project-web.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/project-web.json"
    printf '%s' '{{"schemaVersion":1,"id":"target-workstation","kind":"target_overrides","source":{{"path":"layers/target-workstation.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{"securityPolicy":{{"deniedPaths":["**/.env"],"requiredControls":{{"secret_scan":true}},"allowlists":{{"git_hosts":["github.com","evil.example"]}},"minimums":{{"backup_count":1}},"maximums":{{"snapshot_age_hours":24}}}}}}}}' > "$4/layers/target-workstation.json"
    exit 0 ;;
esac
exit 91
"#,
            log.display()
        ),
    );
    test_support::write_tool(
        &bin,
        "git",
        &format!(
            "printf 'git %s\\n' \"$*\" >> '{}'\n[ \"$3\" = rev-parse ] && printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\nexit 0",
            log.display()
        ),
    );
    let mut request = request(temporary.path(), InitMode::Connect, "owner/unsafe-kit");
    request.project_loadout = Some("project-web".to_owned());
    request.target_override = Some("target-workstation".to_owned());

    let error = initialize(&request, &ProcessRunner::new(&bin)).unwrap_err();

    assert!(error.to_string().contains("allowlist"));
    assert!(
        !request
            .kit_directory
            .join("targets/workstation.json")
            .exists()
    );
    assert!(!request.state_directory.join("plans").exists());
    assert!(!request.config_directory.join("headless.json").exists());
    let commands = std::fs::read_to_string(log).unwrap();
    assert!(!commands.contains("git add"));
    assert!(!commands.contains("git commit"));
    assert!(!commands.contains("git push"));
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn connect_with_apm_validates_the_pin_and_writes_a_provider_pipeline() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("target")).unwrap();
    test_support::write_tool(
        &bin,
        "apm",
        r#"
if [ "$1" = "--version" ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0\n'; exit 0; fi
if [ "$1" = "install" ]; then exit 0; fi
if [ "$1" = "compile" ]; then mkdir -p .claude .codex; printf 'claude\n' > .claude/CLAUDE.md; printf 'codex\n' > .codex/AGENTS.md; exit 0; fi
if [ "$1" = "audit" ]; then printf '{}\n'; exit 0; fi
exit 93
"#,
    );
    test_support::write_tool(
        &bin,
        "gh",
        r#"
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers" "$4/agent/.apm"
    for id in public-base organization-policy personal; do
      kind=personal_kit; [ "$id" = public-base ] && kind=public_base; [ "$id" = organization-policy ] && kind=organization_policy
      printf '{"schemaVersion":1,"id":"%s","kind":"%s","source":{"path":"layers/%s.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' "$id" "$kind" "$id" > "$4/layers/$id.json"
    done
    printf 'name: kit\n' > "$4/agent/apm.yml"
    printf 'lockfileVersion: 1\n' > "$4/agent/apm.lock.yaml"
    printf 'allowedSources: []\n' > "$4/agent/apm-policy.yml"
    exit 0 ;;
esac
exit 91
"#,
    );
    test_support::write_tool(
        &bin,
        "git",
        "[ \"$3\" = rev-parse ] && printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\nexit 0",
    );
    let mut request = request(temporary.path(), InitMode::Connect, "owner/kit");
    request.provider = ProviderSelection::Apm {
        executable: bin.join("apm"),
        manifest: "agent/apm.yml".into(),
        lockfile: "agent/apm.lock.yaml".into(),
        policy: "agent/apm-policy.yml".into(),
    };
    let result = initialize(&request, &ProcessRunner::new(&bin)).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(result.headless_config).unwrap()).unwrap();
    let provider = &config["sync"]["providerPipeline"]["providers"][0];
    assert_eq!(provider["provider"], "apm");
    assert_eq!(provider["version"], "0.25.0");
    assert_eq!(provider["manifest"], "agent/apm.yml");
    assert_eq!(
        config["sync"]["materializedStates"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn connect_uses_existing_gh_credentials_and_writes_first_run_state() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("target")).unwrap();
    std::fs::create_dir_all(temporary.path().join("config")).unwrap();
    std::fs::create_dir_all(temporary.path().join("state")).unwrap();
    std::fs::write(temporary.path().join("config/daemon-token"), b"keep-config").unwrap();
    std::fs::write(temporary.path().join("state/service-state"), b"keep-state").unwrap();
    std::fs::set_permissions(
        temporary.path().join("target"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    test_support::write_tool(
        &bin,
        "gh",
        r#"
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    [ -e "$4" ] && exit 92
    mkdir -p "$4/.git"
    mkdir -p "$4/layers"
    printf '{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/public-base.json"
    printf '{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{}}' > "$4/layers/organization-policy.json"
    mkdir -p "$4/portable"
    printf 'managed from git\n' > "$4/portable/editor.conf"
    printf '{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"files":[{"path":"portable/editor.conf","source":"portable/editor.conf"}]}}' > "$4/layers/personal.json"
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
  remote) printf 'https://github.com/unsoldgroup/commonkit-config.git\n'; exit 0 ;;
  status) exit 0 ;;
esac
exit 0
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
        0
    );
    let serialized = serde_json::to_string(&config).unwrap();
    assert!(!serialized.to_ascii_lowercase().contains("token"));
    assert!(!serialized.contains("ghp_"));

    let plans = std::sync::Arc::new(
        commonkit_reconcile::PlanStore::open(temporary.path().join("state/plans")).unwrap(),
    );
    let registry = commonkit_service::ProductionDomainRegistry::load(
        &result.headless_config,
        plans.clone(),
        temporary.path().join("receipts"),
    )
    .unwrap();
    let composition = registry.composition.unwrap().compose().unwrap();
    assert_eq!(composition["spec"]["files"].as_array().unwrap().len(), 1);
    let plan = plans.load(&result.first_plan_id).unwrap();
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(
        plan.operations[0].resource.managed_path.as_deref(),
        Some("portable/editor.conf")
    );
    assert_eq!(
        std::fs::metadata(temporary.path().join("target"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::read(temporary.path().join("config/daemon-token")).unwrap(),
        b"keep-config"
    );
    assert_eq!(
        std::fs::read(temporary.path().join("state/service-state")).unwrap(),
        b"keep-state"
    );

    let retry = initialize(
        &request(
            temporary.path(),
            InitMode::Connect,
            "unsoldgroup/commonkit-config",
        ),
        &ProcessRunner::new(&bin),
    )
    .unwrap();
    assert_eq!(retry.repository_revision, result.repository_revision);
    assert_eq!(retry.first_plan_id, result.first_plan_id);
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
    let selection: serde_json::Value = serde_json::from_slice(
        &std::fs::read(temporary.path().join("config/headless.targets.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(selection["selected"], serde_json::json!(["workstation"]));
    let registration: serde_json::Value = serde_json::from_slice(
        &std::fs::read(result.kit_directory.join("targets/workstation.json")).unwrap(),
    )
    .unwrap();
    assert!(
        !registration
            .as_object()
            .unwrap()
            .contains_key("projectLoadout")
    );
    assert!(
        !registration
            .as_object()
            .unwrap()
            .contains_key("targetOverride")
    );
    let commands = std::fs::read_to_string(log).unwrap();
    assert!(commands.contains("gh repo create owner/new-kit --private"));
    assert!(commands.contains("gh repo clone owner/new-kit"));
    assert!(commands.contains("git -C"));
    assert!(commands.contains("push --set-upstream origin HEAD"));
    assert!(!commands.to_ascii_lowercase().contains("token"));
}

#[test]
fn create_writes_selected_project_and_target_layers_into_the_portable_registration() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    test_support::write_logging_tools(&bin, &temporary.path().join("commands.log"));
    let mut request = request(temporary.path(), InitMode::Create, "owner/five-layer-kit");
    request.project_loadout = Some("project-web".to_owned());
    request.target_override = Some("target-workstation".to_owned());

    let result = initialize(&request, &ProcessRunner::new(&bin)).unwrap();

    let project: LayerDocument = serde_json::from_slice(
        &std::fs::read(result.kit_directory.join("layers/project-web.json")).unwrap(),
    )
    .unwrap();
    let target: LayerDocument = serde_json::from_slice(
        &std::fs::read(result.kit_directory.join("layers/target-workstation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(project.kind, commonkit_contracts::LayerKind::ProjectLoadout);
    assert_eq!(target.kind, commonkit_contracts::LayerKind::TargetOverrides);
    let registration: serde_json::Value = serde_json::from_slice(
        &std::fs::read(result.kit_directory.join("targets/workstation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(registration["loadout"], "personal");
    assert_eq!(registration["projectLoadout"], "project-web");
    assert_eq!(registration["targetOverride"], "target-workstation");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(result.headless_config).unwrap()).unwrap();
    assert_eq!(config["composition"]["layers"].as_array().unwrap().len(), 5);
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn create_imports_apm_inputs_and_materializes_the_first_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let inputs = temporary.path().join("apm-inputs");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&inputs).unwrap();
    std::fs::write(inputs.join("apm.yml"), "name: kit\n").unwrap();
    std::fs::write(inputs.join("apm.lock.yaml"), "lockfileVersion: 1\n").unwrap();
    std::fs::write(inputs.join("apm-policy.yml"), "allowedSources: []\n").unwrap();
    test_support::write_logging_tools(&bin, &temporary.path().join("commands"));
    test_support::write_tool(
        &bin,
        "apm",
        r#"
if [ "$1" = "--version" ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0\n'; exit 0; fi
if [ "$1" = install ]; then exit 0; fi
if [ "$1" = compile ]; then mkdir -p .claude; printf 'context\n' > .claude/CLAUDE.md; exit 0; fi
if [ "$1" = audit ]; then printf '{}\n'; exit 0; fi
exit 93
"#,
    );
    let mut request = request(temporary.path(), InitMode::Create, "owner/apm-kit");
    request.provider = ProviderSelection::Apm {
        executable: bin.join("apm"),
        manifest: inputs.join("apm.yml"),
        lockfile: inputs.join("apm.lock.yaml"),
        policy: inputs.join("apm-policy.yml"),
    };
    let result = initialize(&request, &ProcessRunner::new(&bin)).unwrap();
    assert!(result.kit_directory.join("providers/apm/apm.yml").is_file());
    let plan = commonkit_reconcile::PlanStore::open(temporary.path().join("state/plans"))
        .unwrap()
        .load(&result.first_plan_id)
        .unwrap();
    assert_eq!(plan.operations.len(), 1);
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn create_clones_an_empty_repository_before_importing_provider_files() {
    struct RealCloneRunner {
        remote: std::path::PathBuf,
    }

    impl CommandRunner for RealCloneRunner {
        fn run(&self, program: &str, arguments: &[OsString]) -> Result<String, OnboardingError> {
            let args = arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            if program == "gh" && args.starts_with(&["auth".into(), "status".into()]) {
                return Ok(String::new());
            }
            if program == "gh" && args.starts_with(&["repo".into(), "create".into()]) {
                return Ok(String::new());
            }
            if program == "gh" && args.starts_with(&["repo".into(), "clone".into()]) {
                let output = Command::new("git")
                    .arg("clone")
                    .arg(&self.remote)
                    .arg(&args[3])
                    .output()
                    .unwrap();
                if !output.status.success() {
                    return Err(OnboardingError::ToolFailed {
                        tool: "gh".into(),
                        status: output.status.code(),
                    });
                }
                for (key, value) in [
                    ("user.name", "CommonKit Test"),
                    ("user.email", "commonkit-test@invalid.example"),
                ] {
                    let status = Command::new("git")
                        .arg("-C")
                        .arg(&args[3])
                        .args(["config", "--local", key, value])
                        .status()
                        .unwrap();
                    if !status.success() {
                        return Err(OnboardingError::ToolFailed {
                            tool: "git".into(),
                            status: status.code(),
                        });
                    }
                }
                return Ok(String::new());
            }
            let output = Command::new(program).args(arguments).output().unwrap();
            if !output.status.success() {
                return Err(OnboardingError::ToolFailed {
                    tool: program.into(),
                    status: output.status.code(),
                });
            }
            Ok(String::from_utf8(output.stdout).unwrap())
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let remote = temporary.path().join("remote.git");
    assert!(
        Command::new("git")
            .args(["init", "--bare"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    let inputs = temporary.path().join("apm-inputs");
    std::fs::create_dir_all(&inputs).unwrap();
    std::fs::write(inputs.join("apm.yml"), "name: kit\n").unwrap();
    std::fs::write(inputs.join("apm.lock.yaml"), "lockfileVersion: 1\n").unwrap();
    std::fs::write(inputs.join("apm-policy.yml"), "allowedSources: []\n").unwrap();
    let executable = temporary.path().join("apm");
    test_support::write_tool(
        temporary.path(),
        "apm",
        "[ \"$1\" = --version ] && printf 'Agent Package Manager (APM) CLI version 0.25.0\\n' && exit 0\n[ \"$1\" = install ] && exit 0\n[ \"$1\" = compile ] && mkdir -p .claude && printf context > .claude/CLAUDE.md && exit 0\n[ \"$1\" = audit ] && printf '{}\\n' && exit 0\nexit 93",
    );
    let mut init = request(temporary.path(), InitMode::Create, "owner/real-clone");
    init.provider = ProviderSelection::Apm {
        executable,
        manifest: inputs.join("apm.yml"),
        lockfile: inputs.join("apm.lock.yaml"),
        policy: inputs.join("apm-policy.yml"),
    };
    let result = initialize(&init, &RealCloneRunner { remote }).unwrap();
    assert!(result.kit_directory.join(".git").is_dir());
    assert!(result.kit_directory.join("providers/apm/apm.yml").is_file());
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn create_imports_chezmoi_source_and_rejects_symlinks() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let source = temporary.path().join("chez-source");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("dot_editor"), "managed\n").unwrap();
    let config = temporary.path().join("chezmoi.toml");
    std::fs::write(&config, "[data]\n").unwrap();
    test_support::write_logging_tools(&bin, &temporary.path().join("commands"));
    test_support::write_tool(
        &bin,
        "chezmoi",
        r#"
if [ "$1" = "--version" ]; then printf 'chezmoi version v2.70.4\n'; exit 0; fi
dest=''; src=''; prev=''; for arg in "$@"; do [ "$prev" = --destination ] && dest="$arg"; [ "$prev" = --source ] && src="$arg"; prev="$arg"; done
mkdir -p "$dest"; cp -R "$src"/. "$dest"/; [ -f "$dest/dot_editor" ] && mv "$dest/dot_editor" "$dest/.editor"; exit 0
"#,
    );
    let mut chez_request = request(temporary.path(), InitMode::Create, "owner/chez-kit");
    chez_request.provider = ProviderSelection::Chezmoi {
        executable: bin.join("chezmoi"),
        source: source.clone(),
        config: config.clone(),
    };
    let result = initialize(&chez_request, &ProcessRunner::new(&bin)).unwrap();
    assert!(
        result
            .kit_directory
            .join("providers/chezmoi/source/dot_editor")
            .is_file()
    );
    assert!(
        commonkit_reconcile::PlanStore::open(temporary.path().join("state/plans"))
            .unwrap()
            .load(&result.first_plan_id)
            .is_ok()
    );

    let unsafe_root = tempfile::tempdir().unwrap();
    let unsafe_source = unsafe_root.path().join("source");
    std::fs::create_dir_all(&unsafe_source).unwrap();
    std::os::unix::fs::symlink(&config, unsafe_source.join("dot_escape")).unwrap();
    let mut unsafe_request = request(unsafe_root.path(), InitMode::Create, "owner/unsafe-kit");
    unsafe_request.provider = ProviderSelection::Chezmoi {
        executable: bin.join("chezmoi"),
        source: unsafe_source,
        config,
    };
    let error = initialize(&unsafe_request, &ProcessRunner::new(&bin)).unwrap_err();
    assert!(error.to_string().contains("symlink"));
}

#[test]
fn create_rejects_secret_plaintext_in_provider_imports() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let source = temporary.path().join("source");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("dot_config"), "api_token: plaintext-value\n").unwrap();
    let config = temporary.path().join("chezmoi.toml");
    std::fs::write(&config, "[data]\n").unwrap();
    test_support::write_logging_tools(&bin, &temporary.path().join("commands"));
    test_support::write_tool(&bin, "chezmoi", "exit 0");
    let mut request = request(temporary.path(), InitMode::Create, "owner/secret-kit");
    request.provider = ProviderSelection::Chezmoi {
        executable: bin.join("chezmoi"),
        source,
        config,
    };
    let error = initialize(&request, &ProcessRunner::new(&bin)).unwrap_err();
    assert!(error.to_string().contains("secret-like"));
}

#[test]
fn create_rejects_non_utf8_provider_inputs_before_clone_or_copy() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let source = temporary.path().join("source");
    let log = temporary.path().join("commands");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("dot_credentials"),
        b"api_key=ghp_binary_payload\xff\xfe",
    )
    .unwrap();
    let config = temporary.path().join("chezmoi.toml");
    std::fs::write(&config, "[data]\n").unwrap();
    test_support::write_logging_tools(&bin, &log);
    test_support::write_tool(&bin, "chezmoi", "exit 0");
    let mut init = request(
        temporary.path(),
        InitMode::Create,
        "owner/binary-secret-kit",
    );
    init.provider = ProviderSelection::Chezmoi {
        executable: bin.join("chezmoi"),
        source,
        config,
    };

    let error = initialize(&init, &ProcessRunner::new(&bin)).unwrap_err();
    assert!(error.to_string().contains("UTF-8 text"), "{error}");
    assert!(!init.kit_directory.exists());
    assert!(
        !log.exists()
            || !std::fs::read_to_string(log)
                .unwrap()
                .contains("repo create")
    );
}

#[test]
fn create_removes_the_local_checkout_when_provider_validation_fails() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let inputs = temporary.path().join("apm-inputs");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&inputs).unwrap();
    std::fs::write(inputs.join("apm.yml"), "name: kit\n").unwrap();
    std::fs::write(inputs.join("apm.lock.yaml"), "lockfileVersion: 1\n").unwrap();
    std::fs::write(inputs.join("apm-policy.yml"), "allowedSources: []\n").unwrap();
    let log = temporary.path().join("commands");
    test_support::write_logging_tools(&bin, &log);
    test_support::write_tool(
        &bin,
        "apm",
        "[ \"$1\" = --version ] && printf 'Agent Package Manager (APM) CLI version 9.9.9\\n' && exit 0\nexit 93",
    );
    let mut init = request(temporary.path(), InitMode::Create, "owner/failed-kit");
    init.provider = ProviderSelection::Apm {
        executable: bin.join("apm"),
        manifest: inputs.join("apm.yml"),
        lockfile: inputs.join("apm.lock.yaml"),
        policy: inputs.join("apm-policy.yml"),
    };

    assert!(initialize(&init, &ProcessRunner::new(&bin)).is_err());
    assert!(!init.kit_directory.exists());
    assert!(!std::fs::read_to_string(log).unwrap().contains("push"));
}

#[test]
fn create_push_failure_leaves_no_local_runtime_referencing_the_removed_checkout() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands");
    std::fs::create_dir_all(&bin).unwrap();
    test_support::write_logging_tools(&bin, &log);
    test_support::write_tool(
        &bin,
        "git",
        &format!(
            r#"
printf 'git %s\n' "$*" >> '{}'
if [ "$3" = "rev-parse" ]; then printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'; exit 0; fi
if [ "$3" = "push" ]; then exit 42; fi
exit 0
"#,
            log.display()
        ),
    );
    let init = request(temporary.path(), InitMode::Create, "owner/push-fails");

    let error = initialize(&init, &ProcessRunner::new(&bin)).unwrap_err();

    assert!(error.to_string().contains("status Some(42)"), "{error}");
    assert!(!init.kit_directory.exists());
    assert!(!init.config_directory.exists());
    assert!(!init.state_directory.exists());
    assert!(!init.target_root.exists());
}

#[test]
fn connect_rejects_native_sources_that_escape_the_git_checkout() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(temporary.path().join("outside"), b"must not import").unwrap();
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
    printf '{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"spec":{"files":[{"path":"portable/escape","source":"portable/escape"}]}}' > "$4/layers/personal.json"
    mkdir "$4/portable"
    ln -s "../../outside" "$4/portable/escape"
    exit 0 ;;
esac
exit 91
"#,
    );
    test_support::write_tool(
        &bin,
        "git",
        "[ \"$3\" = rev-parse ] && printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\nexit 0",
    );
    let error = initialize(
        &request(temporary.path(), InitMode::Connect, "owner/kit"),
        &ProcessRunner::new(&bin),
    )
    .unwrap_err();
    assert!(error.to_string().contains("regular non-symlink file"));
}

#[test]
fn connect_provider_failure_does_not_publish_registration_or_local_runtime() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands");
    std::fs::create_dir_all(&bin).unwrap();
    test_support::write_tool(
        &bin,
        "gh",
        &format!(
            r#"
printf 'gh %s\n' "$*" >> '{}'
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers"
    printf '{{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/public-base.json"
    printf '{{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/organization-policy.json"
    printf '{{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{"files":[{{"path":"portable/missing","source":"portable/missing"}}]}}}}' > "$4/layers/personal.json"
    exit 0 ;;
esac
exit 91
"#,
            log.display()
        ),
    );
    test_support::write_tool(
        &bin,
        "git",
        &format!(
            "printf 'git %s\\n' \"$*\" >> '{}'\n[ \"$3\" = rev-parse ] && printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'\nexit 0",
            log.display()
        ),
    );
    let init = request(temporary.path(), InitMode::Connect, "owner/provider-fails");

    let error = initialize(&init, &ProcessRunner::new(&bin)).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("native provider source is missing")
    );
    let commands = std::fs::read_to_string(&log).unwrap();
    assert!(!commands.contains("git add"));
    assert!(!commands.contains("git commit"));
    assert!(!commands.contains("git push"));
    assert!(!init.kit_directory.join("targets/workstation.json").exists());
    assert!(!init.config_directory.exists());
    assert!(!init.state_directory.exists());
}

#[test]
fn connect_runtime_failure_after_commit_rolls_back_registration_and_preserves_local_runtime() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands");
    let committed = temporary.path().join("committed");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("config")).unwrap();
    std::fs::create_dir_all(temporary.path().join("state")).unwrap();
    std::fs::write(temporary.path().join("config/daemon-token"), b"keep-config").unwrap();
    std::fs::write(temporary.path().join("state/service-state"), b"keep-state").unwrap();
    test_support::write_tool(
        &bin,
        "gh",
        &format!(
            r#"
printf 'gh %s\n' "$*" >> '{}'
case "$1 $2" in
  "auth status") exit 0 ;;
  "repo clone")
    mkdir -p "$4/layers" "$4/portable"
    printf 'managed\n' > "$4/portable/editor.conf"
    printf '{{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/public-base.json"
    printf '{{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{}}}}' > "$4/layers/organization-policy.json"
    printf '{{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},"spec":{{"files":[{{"path":"portable/editor.conf","source":"portable/editor.conf"}}]}}}}' > "$4/layers/personal.json"
    exit 0 ;;
esac
exit 91
"#,
            log.display()
        ),
    );
    test_support::write_tool(
        &bin,
        "git",
        &format!(
            r#"
printf 'git %s\n' "$*" >> '{}'
if [ "$3" = rev-parse ]; then
  if [ -f '{}' ]; then printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n'; else printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'; fi
  exit 0
fi
if [ "$3" = commit ]; then touch '{}'; rm -f '{}'; exit 0; fi
exit 0
"#,
            log.display(),
            committed.display(),
            committed.display(),
            temporary.path().join("kit/portable/editor.conf").display()
        ),
    );
    let init = request(temporary.path(), InitMode::Connect, "owner/runtime-fails");

    let error = initialize(&init, &ProcessRunner::new(&bin)).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("native provider source is missing")
    );
    let commands = std::fs::read_to_string(&log).unwrap();
    assert!(commands.contains("git -C"));
    assert!(commands.contains("reset --hard aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(!commands.contains("git -C") || !commands.contains(" push origin HEAD"));
    assert!(!init.kit_directory.join("targets/workstation.json").exists());
    assert_eq!(
        std::fs::read(temporary.path().join("config/daemon-token")).unwrap(),
        b"keep-config"
    );
    assert_eq!(
        std::fs::read(temporary.path().join("state/service-state")).unwrap(),
        b"keep-state"
    );
    assert!(!init.config_directory.join("headless.json").exists());
    assert!(!init.state_directory.join("plans").exists());
}

#[test]
fn connect_push_failure_restores_registration_and_leaves_existing_runtime_unchanged() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands");
    let committed = temporary.path().join("committed");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("config")).unwrap();
    std::fs::create_dir_all(temporary.path().join("state/plans")).unwrap();
    std::fs::write(temporary.path().join("config/headless.json"), b"old-config").unwrap();
    std::fs::write(temporary.path().join("state/plans/old-plan"), b"old-plan").unwrap();
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
        &format!(
            r#"
printf 'git %s\n' "$*" >> '{}'
if [ "$3" = rev-parse ]; then
  if [ -f '{}' ]; then printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n'; else printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'; fi
  exit 0
fi
if [ "$3" = commit ]; then touch '{}'; exit 0; fi
if [ "$3" = push ]; then exit 42; fi
exit 0
"#,
            log.display(),
            committed.display(),
            committed.display()
        ),
    );
    let init = request(temporary.path(), InitMode::Connect, "owner/push-fails");

    let error = initialize(&init, &ProcessRunner::new(&bin)).unwrap_err();

    assert!(error.to_string().contains("status Some(42)"), "{error}");
    let commands = std::fs::read_to_string(&log).unwrap();
    assert!(commands.contains("push origin HEAD"));
    assert!(commands.contains("reset --hard aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(!init.kit_directory.join("targets/workstation.json").exists());
    assert_eq!(
        std::fs::read(temporary.path().join("config/headless.json")).unwrap(),
        b"old-config"
    );
    assert_eq!(
        std::fs::read(temporary.path().join("state/plans/old-plan")).unwrap(),
        b"old-plan"
    );
}

#[test]
fn connect_runtime_publication_failure_happens_before_push_and_rolls_back_local_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    let log = temporary.path().join("commands");
    let committed = temporary.path().join("committed");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(temporary.path().join("config")).unwrap();
    std::fs::create_dir_all(temporary.path().join("state")).unwrap();
    std::fs::write(temporary.path().join("config/daemon-token"), b"keep-config").unwrap();
    std::fs::write(temporary.path().join("state/providers"), b"blocking-file").unwrap();
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
        &format!(
            r#"
printf 'git %s\n' "$*" >> '{}'
if [ "$3" = rev-parse ]; then
  if [ -f '{}' ]; then printf 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n'; else printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n'; fi
  exit 0
fi
if [ "$3" = commit ]; then touch '{}'; exit 0; fi
exit 0
"#,
            log.display(),
            committed.display(),
            committed.display()
        ),
    );
    let init = request(
        temporary.path(),
        InitMode::Connect,
        "owner/publication-fails",
    );

    let error = initialize(&init, &ProcessRunner::new(&bin)).unwrap_err();

    assert!(
        error.to_string().contains("unsafe onboarding path"),
        "{error}"
    );
    let commands = std::fs::read_to_string(&log).unwrap();
    assert!(!commands.contains("push origin HEAD"));
    assert!(commands.contains("reset --hard aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(!init.kit_directory.join("targets/workstation.json").exists());
    assert_eq!(
        std::fs::read(temporary.path().join("config/daemon-token")).unwrap(),
        b"keep-config"
    );
    assert_eq!(
        std::fs::read(temporary.path().join("state/providers")).unwrap(),
        b"blocking-file"
    );
    assert!(!init.config_directory.join("headless.json").exists());
    assert!(!init.target_root.exists());
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

#[test]
fn permits_platform_state_directory_nested_under_private_config_root() {
    let temporary = tempfile::tempdir().unwrap();
    let mut request = request(temporary.path(), InitMode::Connect, "owner/kit");
    request.config_directory = temporary
        .path()
        .join("Library/Application Support/CommonKit");
    request.state_directory = request.config_directory.join("state");
    let error = initialize(&request, &ProcessRunner::new(temporary.path())).unwrap_err();
    assert!(!error.to_string().contains("must not overlap"));
}

#[test]
fn connect_requires_publish_consent_before_clone_or_registration_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let mut request = request(temporary.path(), InitMode::Connect, "owner/kit");
    request.publish_registration = false;
    let error = initialize(&request, &ProcessRunner::new(temporary.path())).unwrap_err();
    assert!(error.to_string().contains("--publish-registration"));
    assert!(!request.kit_directory.exists());
}

#[test]
fn create_requires_publish_consent_before_repository_creation() {
    let temporary = tempfile::tempdir().unwrap();
    let mut request = request(temporary.path(), InitMode::Create, "owner/kit");
    request.publish_registration = false;
    let error = initialize(&request, &ProcessRunner::new(temporary.path())).unwrap_err();
    assert!(error.to_string().contains("--publish-registration"));
    assert!(!request.kit_directory.exists());
}

mod test_support {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    pub fn write_tool(bin: &Path, name: &str, body: &str) {
        write_tool_inner(bin, name, body, true);
    }

    pub fn write_raw_tool(bin: &Path, name: &str, body: &str) {
        write_tool_inner(bin, name, body, false);
    }

    fn write_tool_inner(bin: &Path, name: &str, body: &str, seal: bool) {
        let path = bin.join(name);
        let seal_layers = if seal && name == "gh" {
            r#"
status=$?
if [ "$1 $2" = "repo clone" ] && [ "$status" -eq 0 ] && [ -d "$4/layers" ]; then
  node - "$4/layers" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const canonical = (value) => Array.isArray(value)
  ? `[${value.map(canonical).join(",")}]`
  : value && typeof value === "object"
    ? `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`
    : JSON.stringify(value);
for (const name of fs.readdirSync(process.argv[2])) {
  if (!name.endsWith(".json")) continue;
  const file = path.join(process.argv[2], name);
  const document = JSON.parse(fs.readFileSync(file, "utf8"));
  delete document.source.contentDigest;
  const digest = crypto.createHash("sha256")
    .update("commonkit.layer-content.v1").update(Buffer.from([0]))
    .update(canonical(document)).digest("hex");
  document.source.contentDigest = `sha256:${digest}`;
  fs.writeFileSync(file, JSON.stringify(document));
}
NODE
fi
exit "$status"
"#
        } else {
            ""
        };
        std::fs::write(&path, format!("#!/bin/sh\n(\n{body}\n)\n{seal_layers}")).unwrap();
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
