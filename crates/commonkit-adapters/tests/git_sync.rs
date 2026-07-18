use std::collections::BTreeMap;

use commonkit_adapters::{
    FastForwardPolicy, GitCommandError, GitCommandOutput, GitCommandRunner, GitRepository,
    GitSyncDisposition, GitSyncError,
};

#[derive(Default)]
struct FakeGit {
    outputs: BTreeMap<String, Result<GitCommandOutput, GitCommandError>>,
    calls: Vec<Vec<String>>,
}

impl FakeGit {
    fn stdout(mut self, args: &[&str], stdout: &str) -> Self {
        self.outputs.insert(
            args.join("\0"),
            Ok(GitCommandOutput {
                stdout: stdout.into(),
            }),
        );
        self
    }
}

impl GitCommandRunner for FakeGit {
    fn run(&mut self, args: &[String]) -> Result<GitCommandOutput, GitCommandError> {
        self.calls.push(args.to_vec());
        self.outputs
            .remove(&args.join("\0"))
            .unwrap_or_else(|| panic!("unexpected git invocation: {args:?}"))
    }
}

fn inspected_repo(divergence: &str, status: &str) -> GitRepository<FakeGit> {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let runner = FakeGit::default()
        .stdout(&["fetch", "--prune", "--no-tags", "origin"], "")
        .stdout(
            &["status", "--porcelain=v1", "--untracked-files=all"],
            status,
        )
        .stdout(&["rev-parse", "--verify", "HEAD"], head)
        .stdout(&["symbolic-ref", "--quiet", "--short", "HEAD"], "main\n")
        .stdout(
            &["config", "--get", "remote.origin.url"],
            "git@github.com:unsoldgroup/commonkit-store.git\n",
        )
        .stdout(&["rev-parse", "--verify", "refs/remotes/origin/main"], head)
        .stdout(
            &[
                "rev-list",
                "--left-right",
                "--count",
                "HEAD...refs/remotes/origin/main",
            ],
            divergence,
        )
        .stdout(
            &["ls-tree", "-r", "--full-tree", "HEAD"],
            "100644 blob abc\tcommonkit.yml\n",
        )
        .stdout(
            &["ls-tree", "-r", "--full-tree", "refs/remotes/origin/main"],
            "100644 blob def\tcommonkit.yml\n",
        );
    GitRepository::new(
        runner,
        "git@github.com:unsoldgroup/commonkit-store.git",
        "origin",
    )
}

#[test]
fn inspection_classifies_clean_dirty_ahead_behind_and_diverged() {
    let cases = [
        ("0\t0\n", "", GitSyncDisposition::Clean),
        ("0\t0\n", " M commonkit.yml\n", GitSyncDisposition::Dirty),
        ("2\t0\n", "", GitSyncDisposition::Ahead),
        ("0\t3\n", "", GitSyncDisposition::Behind),
        ("2\t3\n", "", GitSyncDisposition::Diverged),
    ];
    for (divergence, worktree, expected) in cases {
        let mut repo = inspected_repo(divergence, worktree);
        let status = repo.inspect(false).unwrap();
        assert_eq!(status.disposition, expected);
        assert_eq!(
            status.revision.as_str(),
            "0123456789abcdef0123456789abcdef01234567"
        );
    }
}

#[test]
fn fetch_only_inspection_uses_fixed_arguments_and_does_not_merge_or_apply() {
    let mut repo = inspected_repo("0\t1\n", "");

    assert_eq!(
        repo.inspect(true).unwrap().disposition,
        GitSyncDisposition::Behind
    );
    assert!(
        repo.runner()
            .calls
            .iter()
            .any(|args| args == &["fetch", "--prune", "--no-tags", "origin"])
    );
    assert!(!repo.runner().calls.iter().any(|args| {
        args.first()
            .is_some_and(|arg| arg == "merge" || arg == "apply")
    }));
}

#[test]
fn approved_fast_forward_uses_ff_only_and_returns_the_fetched_revision_without_applying() {
    let mut repo = inspected_repo("0\t1\n", "");
    repo.runner_mut().outputs.insert(
        ["merge", "--ff-only", "refs/remotes/origin/main"].join("\0"),
        Ok(GitCommandOutput {
            stdout: "Updating files".into(),
        }),
    );

    let status = repo
        .fast_forward(FastForwardPolicy { enabled: true }, |_| true)
        .unwrap();

    assert_eq!(status.disposition, GitSyncDisposition::Clean);
    assert!(
        repo.runner()
            .calls
            .iter()
            .any(|args| args == &["merge", "--ff-only", "refs/remotes/origin/main"])
    );
    assert!(
        !repo
            .runner()
            .calls
            .iter()
            .any(|args| args.iter().any(|arg| arg == "apply"))
    );
}

#[test]
fn untrusted_remote_is_rejected_before_fetch() {
    let mut repo = inspected_repo("0\t0\n", "");
    repo.runner_mut().outputs.insert(
        ["config", "--get", "remote.origin.url"].join("\0"),
        Ok(GitCommandOutput {
            stdout: "https://attacker.invalid/kit.git\n".into(),
        }),
    );

    let error = repo.inspect(true).unwrap_err();

    assert!(matches!(error, GitSyncError::UntrustedRemote { .. }));
    assert!(
        !repo
            .runner()
            .calls
            .iter()
            .any(|args| args.first().is_some_and(|arg| arg == "fetch"))
    );
}

#[test]
fn guarded_fast_forward_requires_opt_in_and_organization_approval() {
    let mut repo = inspected_repo("0\t1\n", "");
    let error = repo
        .fast_forward(FastForwardPolicy { enabled: false }, |_| true)
        .unwrap_err();
    assert!(matches!(error, GitSyncError::FastForwardDisabled));

    let mut repo = inspected_repo("0\t1\n", "");
    let error = repo
        .fast_forward(FastForwardPolicy { enabled: true }, |_| false)
        .unwrap_err();
    assert!(matches!(error, GitSyncError::OrganizationPolicyRejected));
}

#[test]
fn prospective_revision_rejects_symlinks_submodules_and_forbidden_portable_paths() {
    for tree in [
        "120000 blob abc\tconfig/link\n",
        "160000 commit abc\tvendor/module\n",
        "100644 blob abc\t.env\n",
        "100644 blob abc\tstate_3.sqlite\n",
    ] {
        let mut repo = inspected_repo("0\t1\n", "");
        repo.runner_mut().outputs.insert(
            ["ls-tree", "-r", "--full-tree", "refs/remotes/origin/main"].join("\0"),
            Ok(GitCommandOutput {
                stdout: tree.into(),
            }),
        );
        let error = repo
            .fast_forward(FastForwardPolicy { enabled: true }, |_| true)
            .unwrap_err();
        assert!(
            matches!(error, GitSyncError::UnsafeTree { .. }),
            "{error:?}"
        );
    }
}
