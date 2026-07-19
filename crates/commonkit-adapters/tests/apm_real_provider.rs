use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, DesiredStateProvider, ExactProviderVersion,
    NormalizedManagedPath, ProviderContext, ProviderWorkspace, redacted_apm_diagnostic_summary,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

#[test]
#[ignore = "requires checksum-verified APM 0.25.0 binary"]
fn checksum_pinned_apm_release_materializes_without_touching_live_target() {
    let executable = PathBuf::from(
        std::env::var_os("COMMONKIT_APM_025_BIN")
            .expect("CI must provide the checksum-verified APM 0.25.0 binary"),
    );
    let root = std::env::temp_dir().join(format!(
        "commonkit-apm-real-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join(".apm/instructions")).unwrap();
    fs::write(root.join("apm.yml"), "name: commonkit-ci\nversion: 1.0.0\ntargets: [claude, codex]\nincludes:\n  - .apm/instructions/\ndependencies:\n  apm: []\n  mcp: []\n").unwrap();
    fs::write(root.join("apm.lock.yaml"), "lockfile_version: '1'\ngenerated_at: '2026-07-18T00:00:00+00:00'\napm_version: 0.25.0\ndependencies: []\ndeployments: []\n").unwrap();
    fs::write(root.join("apm-policy.yml"), "version: 1\n").unwrap();
    fs::write(root.join(".apm/instructions/base.instructions.md"), "---\ndescription: CommonKit CI fixture\napplyTo: \"**\"\n---\n# Fixture\n\nStay in staging.\n").unwrap();
    let stage = root.join("stage");
    let live = root.join("live");
    fs::create_dir_all(&stage).unwrap();
    fs::create_dir_all(&live).unwrap();
    let provider = ApmProvider::new(ApmProviderConfig {
        executable,
        git_executable: None,
        version: ExactProviderVersion::parse("0.25.0").unwrap(),
        manifest: root.join("apm.yml"),
        lockfile: root.join("apm.lock.yaml"),
        policy: root.join("apm-policy.yml"),
        targets: vec!["claude".into(), "codex".into()],
        managed_root: NormalizedManagedPath::parse("home").unwrap(),
        bound_source: None,
    })
    .unwrap();
    let context = ProviderContext {
        target_id: StableId::parse("ci").unwrap(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        policy_digest: digest('f'),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::new(),
    };
    let workspace = ProviderWorkspace::open(&stage, std::slice::from_ref(&live)).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let state = provider
        .materialize(&context, &workspace, &artifacts)
        .unwrap_or_else(|error| {
            let summary = redacted_apm_diagnostic_summary(workspace.scratch_root(), "install")
                .unwrap_or_else(|_| "private diagnostic unavailable".into());
            panic!("{error}\nredacted bounded APM install diagnostic:\n{summary}");
        });
    state.verify().unwrap();
    assert!(
        state
            .resources
            .iter()
            .any(|resource| resource.intent.path().as_str() == "home/AGENTS.md")
    );
    assert!(
        state.resources.iter().any(|resource| resource
            .intent
            .path()
            .as_str()
            .starts_with("home/.claude/"))
    );
    assert_eq!(fs::read_dir(live).unwrap().count(), 0);
    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}
