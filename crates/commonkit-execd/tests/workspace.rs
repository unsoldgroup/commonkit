#![allow(clippy::unwrap_used)]
use commonkit_contracts::*;
use commonkit_execd::workspace;
use std::collections::BTreeSet;
use std::process::Command;

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn manifest(repository: &str, revision: GitRevision) -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: repository.to_owned(),
        repository_revision: revision,
        workspace_bundle_digest: None,
        argv: vec!["true".into()],
        workdir: PortableSourcePath::parse("work").unwrap(),
        secret_refs: Vec::new(),
        timeout_seconds: 30,
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
            max_attempts: 1,
            retryable_exit_codes: BTreeSet::new(),
        },
        checkpoint_enabled: false,
        artifacts: ArtifactPolicy {
            globs: vec!["diagnostics/**".into()],
            retention_seconds: 60,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Deny,
        repository_write: false,
        browser: None,
    }
}

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Builds a source repository with two commits and returns (path, first, second).
fn source_repository(root: &std::path::Path) -> (String, GitRevision, GitRevision) {
    std::fs::create_dir_all(root.join("work")).unwrap();
    std::fs::write(root.join("work/marker"), "first").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "tests@commonkit.invalid"],
        vec!["config", "user.name", "CommonKit Tests"],
        vec!["add", "."],
        vec!["commit", "-qm", "first"],
    ] {
        git(root, &args);
    }
    let first = GitRevision::parse(git(root, &["rev-parse", "HEAD"])).unwrap();
    std::fs::write(root.join("work/marker"), "second").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "second"]);
    let second = GitRevision::parse(git(root, &["rev-parse", "HEAD"])).unwrap();
    (root.to_str().unwrap().to_owned(), first, second)
}

#[test]
fn prepared_workspace_matches_the_pinned_revision_and_origin() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let (repository, first, _) = source_repository(&directory.path().join("source"));

    let prepared = workspace::prepare(&manifest(&repository, first.clone()), &root, "job_a").unwrap();

    assert_eq!(git(&prepared, &["rev-parse", "HEAD"]), first.as_str());
    // `verify_workspace` compares the origin URL against `manifest.repository`.
    assert_eq!(git(&prepared, &["remote", "get-url", "origin"]), repository);
    assert!(git(&prepared, &["status", "--porcelain=v1"]).is_empty());
    assert!(prepared.join("work/marker").exists());
}

#[test]
fn two_jobs_hold_different_revisions_of_the_same_repository() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let (repository, first, second) = source_repository(&directory.path().join("source"));

    let older = workspace::prepare(&manifest(&repository, first.clone()), &root, "job_a").unwrap();
    let newer = workspace::prepare(&manifest(&repository, second.clone()), &root, "job_b").unwrap();

    assert_ne!(older, newer);
    assert_eq!(git(&older, &["rev-parse", "HEAD"]), first.as_str());
    assert_eq!(git(&newer, &["rev-parse", "HEAD"]), second.as_str());
    assert_eq!(
        std::fs::read_to_string(older.join("work/marker")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(newer.join("work/marker")).unwrap(),
        "second"
    );
    // One mirror serves both worktrees.
    assert_eq!(std::fs::read_dir(root.join("mirrors")).unwrap().count(), 1);
}

#[test]
fn preparation_fetches_commits_pushed_after_the_mirror_was_created() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let source = directory.path().join("source");
    let (repository, first, _) = source_repository(&source);
    workspace::prepare(&manifest(&repository, first), &root, "job_a").unwrap();

    std::fs::write(source.join("work/marker"), "third").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-qm", "third"]);
    let third = GitRevision::parse(git(&source, &["rev-parse", "HEAD"])).unwrap();

    let prepared = workspace::prepare(&manifest(&repository, third.clone()), &root, "job_b").unwrap();
    assert_eq!(git(&prepared, &["rev-parse", "HEAD"]), third.as_str());
}

#[test]
fn unknown_revision_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let (repository, _, _) = source_repository(&directory.path().join("source"));
    let unknown = GitRevision::parse("a".repeat(40)).unwrap();

    let error = workspace::prepare(&manifest(&repository, unknown), &root, "job_a").unwrap_err();

    assert!(matches!(error, workspace::WorkspaceError::Git { .. }));
    assert!(!root.join("jobs/job_a").exists());
}

#[test]
fn job_identifiers_cannot_escape_the_workspace_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let (repository, first, _) = source_repository(&directory.path().join("source"));

    let error =
        workspace::prepare(&manifest(&repository, first), &root, "../escape").unwrap_err();

    assert!(matches!(error, workspace::WorkspaceError::InvalidJobId(_)));
    assert!(!root.exists());
}

#[test]
fn removal_is_idempotent_and_reclaims_the_worktree() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let (repository, first, _) = source_repository(&directory.path().join("source"));
    let prepared = workspace::prepare(&manifest(&repository, first), &root, "job_a").unwrap();

    workspace::remove(&root, &repository, "job_a").unwrap();
    assert!(!prepared.exists());
    workspace::remove(&root, &repository, "job_a").unwrap();
    assert!(!prepared.exists());
}

#[test]
fn preparation_replaces_a_leftover_worktree_for_the_same_job() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspaces");
    let (repository, first, second) = source_repository(&directory.path().join("source"));

    workspace::prepare(&manifest(&repository, first), &root, "job_a").unwrap();
    let prepared =
        workspace::prepare(&manifest(&repository, second.clone()), &root, "job_a").unwrap();

    assert_eq!(git(&prepared, &["rev-parse", "HEAD"]), second.as_str());
}
