#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, DesiredStateProvider, ExactProviderVersion,
    FilesystemIntent, NormalizedManagedPath, ProviderContext, ProviderWorkspace,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn fixture(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "commonkit-apm-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn context() -> ProviderContext {
    ProviderContext {
        target_id: StableId::parse("local").unwrap(),
        platform: "macos".into(),
        architecture: "aarch64".into(),
        policy_digest: digest('f'),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::new(),
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn configured(root: &Path, executable: PathBuf) -> ApmProvider {
    let manifest = root.join("apm.yml");
    let lockfile = root.join("apm.lock.yaml");
    let policy = root.join("apm-policy.yml");
    fs::write(&manifest, "packages: []\n").unwrap();
    fs::write(&lockfile, "lockfileVersion: 1\npackages: []\n").unwrap();
    fs::write(&policy, "allowedSources: [local]\n").unwrap();
    ApmProvider::new(ApmProviderConfig {
        executable,
        version: ExactProviderVersion::parse("0.25.0").unwrap(),
        manifest,
        lockfile,
        policy,
        targets: vec!["claude".into(), "codex".into()],
        managed_root: NormalizedManagedPath::parse("home").unwrap(),
    })
    .unwrap()
}

#[test]
fn invalid_inputs_fail_before_provider_execution() {
    let root = fixture("preflight");
    let marker = root.join("executed");
    let executable = root.join("apm");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let provider = configured(&root, executable);
    fs::remove_file(root.join("apm.lock.yaml")).unwrap();

    let error = provider.inspect_inputs(&context()).unwrap_err().to_string();
    assert!(error.contains("lockfile"), "{error}");
    assert!(
        !marker.exists(),
        "provider executable ran before preflight passed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_apm_materializes_claude_and_codex_outside_the_live_target() {
    let root = fixture("materialize");
    let log = root.join("argv.log");
    let executable = root.join("apm");
    write_executable(
        &executable,
        &format!(
            r##"#!/bin/sh
printf '%s\n' "$*" >> '{}'
if [ "$1" = "--version" ]; then printf 'apm 0.25.0\n'; exit 0; fi
if [ "$1" = "compile" ]; then
  mkdir -p .claude .codex
  printf 'claude context\n' > .claude/CLAUDE.md
  printf 'codex context\n' > .codex/AGENTS.md
fi
if [ "$1" = "audit" ]; then printf '{{"ok":true}}\n'; fi
"##,
            log.display()
        ),
    );
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    let live = root.join("live");
    let artifacts = root.join("artifacts");
    fs::create_dir_all(&stage).unwrap();
    fs::create_dir_all(&live).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[live.clone()]).unwrap();
    let artifacts = ArtifactStore::open(&artifacts).unwrap();

    let inputs = provider.inspect_inputs(&context()).unwrap();
    assert_eq!(inputs.provider_version.as_str(), "0.25.0");
    assert!(inputs.input_digests.contains_key("manifest"));
    assert!(inputs.input_digests.contains_key("lockfile"));
    assert!(inputs.input_digests.contains_key("packagePolicy"));

    let state = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap();
    state.verify().unwrap();
    assert_eq!(state.resources.len(), 2);
    assert!(state.resources.iter().all(|resource| {
        resource.provenance.provider_id.as_str() == "apm"
            && resource.provenance.provider_version == "0.25.0"
            && resource.provenance.input_digest == *inputs.digest()
            && matches!(resource.intent, FilesystemIntent::File { .. })
    }));
    assert!(!live.join(".claude/CLAUDE.md").exists());
    assert!(!live.join(".codex/AGENTS.md").exists());

    let second_stage = root.join("second-stage");
    fs::create_dir_all(&second_stage).unwrap();
    let second_workspace = ProviderWorkspace::open(&second_stage, &[live.clone()]).unwrap();
    let repeated = provider
        .materialize(&context(), &second_workspace, &artifacts)
        .unwrap();
    assert_eq!(state.digest, repeated.digest);
    assert_eq!(state.resources, repeated.resources);

    let argv = fs::read_to_string(log).unwrap();
    assert!(argv.contains("--version"), "{argv}");
    assert!(
        argv.contains("install --frozen --target claude,codex"),
        "{argv}"
    );
    assert!(argv.contains("compile --target claude,codex"), "{argv}");
    assert!(
        argv.contains(&format!(
            "audit --ci --policy {}/apm-policy.yml --no-fail-fast --format json",
            stage.canonicalize().unwrap().display()
        )),
        "{argv}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn version_mismatch_is_actionable_and_stops_before_compilation() {
    let root = fixture("version");
    let log = root.join("argv.log");
    let executable = root.join("apm");
    write_executable(
        &executable,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf 'apm 0.26.0\\n'\n",
            log.display()
        ),
    );
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected APM 0.25.0"), "{error}");
    assert!(error.contains("found 0.26.0"), "{error}");
    assert_eq!(fs::read_to_string(log).unwrap(), "--version\n");

    fs::remove_dir_all(root).unwrap();
}
