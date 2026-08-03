//! The committed CI declaration must actually place on the committed CI target.
use commonkit_contracts::{ExecutionManifest, ExecutionTarget, digest_domain_json};
use commonkit_execd::ExecutionPolicy;
use commonkit_execution::placement;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionContextFile {
    tasks: BTreeMap<String, ExecutionManifest>,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = repository_root().join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn tasks() -> BTreeMap<String, ExecutionManifest> {
    read::<ExecutionContextFile>("commonkit.execution-context.json").tasks
}

/// The digests a task pins are not free-standing constants: they are the digests
/// of the committed loadout and execution-profile documents. Editing either
/// document without re-pinning stops every declared task from placing, which is
/// the fail-closed behaviour placement exists to give.
#[test]
fn the_pinned_digests_are_the_digests_of_the_committed_documents() {
    let loadout = digest_domain_json(
        "commonkit.loadout.v1",
        &read::<serde_json::Value>("commonkit.ci-loadout.json"),
    )
    .unwrap();
    let profile = digest_domain_json(
        "commonkit.execution-profile.v1",
        &read::<serde_json::Value>("commonkit.execution-profile.json"),
    )
    .unwrap();
    let target: ExecutionTarget = read("commonkit.execution-target.example.json");
    assert_eq!(target.loadout_digest, loadout, "target loadout digest");
    assert_eq!(
        target.execution_profile_digest, profile,
        "target execution profile digest"
    );
    for (id, manifest) in tasks() {
        assert_eq!(manifest.loadout_digest, loadout, "{id} loadout digest");
        assert_eq!(
            manifest.execution_profile_digest, profile,
            "{id} execution profile digest"
        );
    }
}

#[test]
fn every_declared_task_is_a_valid_manifest() {
    let tasks = tasks();
    assert!(!tasks.is_empty());
    for (id, manifest) in &tasks {
        manifest
            .validate()
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        manifest
            .digest()
            .unwrap_or_else(|error| panic!("{id}: {error}"));
    }
}

/// Shell strings are how a task smuggles in arbitrary execution; argv is an array
/// precisely so it cannot.
#[test]
fn declared_tasks_do_not_shell_out() {
    for (id, manifest) in tasks() {
        assert!(
            !matches!(manifest.argv[0].as_str(), "sh" | "bash" | "zsh" | "cmd"),
            "{id} invokes a shell"
        );
    }
}

/// `repositoryWrite` grants a writable bind of the job's own disposable worktree,
/// which every build needs, and grants nothing else: a job cannot author history
/// because no credential reaches it (ADR 0011), and the worktree is removed when
/// the job reaches a terminal state.
#[test]
fn declared_tasks_get_a_writable_workspace_and_the_network_their_toolchain_needs() {
    for (id, manifest) in tasks() {
        assert!(
            manifest.repository_write,
            "{id} cannot write its own build output"
        );
        assert_eq!(
            manifest.network_policy,
            commonkit_contracts::NetworkPolicy::Allow,
            "{id} declares a network policy the supervisor cannot enforce"
        );
        assert!(
            manifest.secret_refs.is_empty(),
            "{id} asks for a secret a public check does not need"
        );
    }
}

#[test]
fn every_declared_task_places_on_the_ci_target() {
    let target: ExecutionTarget = read("commonkit.execution-target.example.json");
    for (id, manifest) in tasks() {
        let result = placement(&manifest, std::slice::from_ref(&target));
        let explanation = &result.explanations[0];
        assert!(
            explanation.eligible,
            "{id} is not placeable: {:?}",
            explanation.reasons
        );
    }
}

#[test]
fn the_execution_policy_admits_every_declared_task() {
    let policy: ExecutionPolicy = read("commonkit.execution-policy.example.json");
    for (id, manifest) in tasks() {
        assert!(
            policy.allowed_repositories.contains(&manifest.repository),
            "{id}: repository not allowed by policy"
        );
        assert!(
            manifest.resources.cpu_millis <= policy.max_cpu_millis
                && manifest.resources.memory_mib <= policy.max_memory_mib
                && manifest.resources.disk_mib <= policy.max_disk_mib,
            "{id}: resources exceed policy ceilings"
        );
        assert!(
            policy.allow_network,
            "{id}: declares a non-deny network policy the execution policy forbids"
        );
    }
}
