//! The committed CI declaration must actually place on the committed CI target.
use commonkit_contracts::{ExecutionManifest, ExecutionTarget};
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
        assert!(
            !manifest.repository_write,
            "{id} requests repository write"
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
