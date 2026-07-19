use commonkit_contracts::*;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
pub fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}
#[allow(dead_code)]
pub fn manifest() -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: "https://example.invalid/repo.git".into(),
        repository_revision: GitRevision::parse("a".repeat(40)).unwrap(),
        workspace_bundle_digest: None,
        argv: vec!["true".into()],
        workdir: PortableSourcePath::parse("work").unwrap(),
        secret_refs: vec![],
        timeout_seconds: 60,
        cancel_grace_seconds: 1,
        resources: ResourceRequirements {
            cpu_millis: 100,
            memory_mib: 64,
            disk_mib: 10,
        },
        required_capabilities: BTreeSet::new(),
        loadout_digest: digest('b'),
        execution_profile_digest: digest('c'),
        retry: RetryPolicy {
            max_attempts: 2,
            retryable_exit_codes: BTreeSet::new(),
        },
        checkpoint_enabled: true,
        artifacts: ArtifactPolicy {
            globs: vec![],
            retention_seconds: 60,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Deny,
        repository_write: false,
        browser: None,
    }
}

#[allow(dead_code)]
pub fn init_repository(root: &Path) -> GitRevision {
    std::fs::write(root.join(".fixture"), "commonkit execution fixture").unwrap();
    std::fs::write(root.join(".gitignore"), "diagnostics/\nlate-marker\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "tests@commonkit.invalid"],
        vec!["config", "user.name", "CommonKit Tests"],
        vec![
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
        vec!["add", "."],
        vec!["commit", "-qm", "fixture"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let output = Command::new("git")
        .args(["-C", root.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    GitRevision::parse(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
}
