use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use commonkit_snapshots::{
    DatabaseId, DeterministicTestCipher, PortableAuthorityFailpoint, PortableAuthorityPublisher,
    PortableAuthorityStore, ProcessGitAuthorityPublisher, SnapshotError,
};
use sha2::{Digest, Sha256};

fn descriptor(root: &std::path::Path, bytes: &[u8]) -> String {
    fs::create_dir_all(root.join("snapshots")).unwrap();
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    fs::write(
        root.join("snapshots")
            .join(format!("{}.json", &digest[7..])),
        bytes,
    )
    .unwrap();
    digest
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[test]
fn promotion_on_machine_b_revokes_machine_a_and_fresh_clone_reads_head() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([71; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store_a = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let initial = store_a.initialize(&database, "machine-a").unwrap();
    let first_descriptor = descriptor(portable.path(), br#"{"snapshot":"one"}"#);
    let head = digest(b"one");
    let first = store_a
        .compare_and_swap_head(
            &database,
            &initial.revision,
            "machine-a",
            &head,
            &first_descriptor,
        )
        .unwrap();

    let store_b = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let promoted = store_b
        .compare_and_swap_writer(&database, &first.revision, "machine-a", "machine-b")
        .unwrap();
    assert_eq!(promoted.record.current_writer, "machine-b");
    assert_eq!(
        promoted.record.accepted_head.as_deref(),
        Some(head.as_str())
    );

    assert_eq!(
        store_a.compare_and_swap_head(
            &database,
            &first.revision,
            "machine-a",
            &digest(b"fork"),
            &first_descriptor,
        ),
        Err(SnapshotError::StalePortableAuthority)
    );

    let clone = tempfile::tempdir().unwrap();
    copy_tree(portable.path(), clone.path());
    let fresh = PortableAuthorityStore::open(clone.path(), &cipher)
        .unwrap()
        .read(&database)
        .unwrap();
    assert_eq!(fresh.record.current_writer, "machine-b");
    assert_eq!(fresh.record.accepted_head.as_deref(), Some(head.as_str()));
    assert_eq!(fresh.revision, promoted.revision);
}

#[test]
fn concurrent_promotions_have_one_cas_winner() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([72; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let current = store.initialize(&database, "machine-a").unwrap();
    let portable = Arc::new(portable);
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for candidate in ["machine-b", "machine-c"] {
        let portable = Arc::clone(&portable);
        let barrier = Arc::clone(&barrier);
        let database = database.clone();
        let revision = current.revision.clone();
        handles.push(thread::spawn(move || {
            let cipher = DeterministicTestCipher::new([72; 32]);
            let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
            barrier.wait();
            store.compare_and_swap_writer(&database, &revision, "machine-a", candidate)
        }));
    }
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(SnapshotError::StalePortableAuthority))
            .count(),
        1
    );
}

#[test]
fn deleting_or_rolling_back_the_accepted_descriptor_fails_closed() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([73; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let initial = store.initialize(&database, "machine-a").unwrap();
    let first_descriptor = descriptor(portable.path(), br#"{"snapshot":"one"}"#);
    store
        .compare_and_swap_head(
            &database,
            &initial.revision,
            "machine-a",
            &digest(b"one"),
            &first_descriptor,
        )
        .unwrap();

    fs::remove_file(
        portable
            .path()
            .join("snapshots")
            .join(format!("{}.json", &first_descriptor[7..])),
    )
    .unwrap();
    assert_eq!(
        store.read(&database),
        Err(SnapshotError::PortableAuthorityRollback)
    );
}

#[test]
fn recomputing_an_unkeyed_digest_cannot_forge_portable_writer_authority() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([74; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    store.initialize(&database, "machine-a").unwrap();

    let path = portable.path().join("authority/context-mode.json");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope["value"]["currentWriter"] = serde_json::json!("attacker");
    envelope["digest"] = serde_json::json!(digest(&serde_jcs::to_vec(&envelope["value"]).unwrap()));
    fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

    assert_eq!(
        store.read(&database),
        Err(SnapshotError::TransactionIntegrity)
    );
}

#[test]
fn replaying_an_older_authenticated_authority_is_rejected_when_history_remains() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([75; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let initial = store.initialize(&database, "machine-a").unwrap();
    let old_authority = fs::read(portable.path().join("authority/context-mode.json")).unwrap();
    let first_descriptor = descriptor(portable.path(), br#"{"snapshot":"one"}"#);
    store
        .compare_and_swap_head(
            &database,
            &initial.revision,
            "machine-a",
            &digest(b"one"),
            &first_descriptor,
        )
        .unwrap();

    fs::write(
        portable.path().join("authority/context-mode.json"),
        old_authority,
    )
    .unwrap();

    assert_eq!(
        store.read(&database),
        Err(SnapshotError::PortableAuthorityRollback)
    );
}

#[test]
fn replaying_an_entire_old_portable_tree_is_rejected_by_the_independent_anchor() {
    let portable = tempfile::tempdir().unwrap();
    let anchor = tempfile::tempdir().unwrap();
    let old_tree = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([76; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store =
        PortableAuthorityStore::open_with_trusted_anchor(portable.path(), anchor.path(), &cipher)
            .unwrap();
    let initial = store.initialize(&database, "machine-a").unwrap();
    copy_tree(portable.path(), old_tree.path());
    store
        .compare_and_swap_writer(&database, &initial.revision, "machine-a", "machine-b")
        .unwrap();

    fs::remove_dir_all(portable.path()).unwrap();
    fs::create_dir_all(portable.path()).unwrap();
    copy_tree(old_tree.path(), portable.path());

    let restarted =
        PortableAuthorityStore::open_with_trusted_anchor(portable.path(), anchor.path(), &cipher)
            .unwrap();
    assert_eq!(
        restarted.read(&database),
        Err(SnapshotError::PortableAuthorityRollback)
    );
}

#[test]
fn restart_repairs_a_torn_history_before_pointer_publication() {
    let portable = tempfile::tempdir().unwrap();
    let anchor = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([77; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store =
        PortableAuthorityStore::open_with_trusted_anchor(portable.path(), anchor.path(), &cipher)
            .unwrap();
    let initial = store.initialize(&database, "machine-a").unwrap();
    assert_eq!(
        store
            .compare_and_swap_writer_with_failpoint(
                &database,
                &initial.revision,
                "machine-a",
                "machine-b",
                PortableAuthorityFailpoint::AfterHistoryWrite,
            )
            .unwrap_err(),
        SnapshotError::Interrupted
    );

    let repaired =
        PortableAuthorityStore::open_with_trusted_anchor(portable.path(), anchor.path(), &cipher)
            .unwrap()
            .read(&database)
            .unwrap();
    assert_eq!(repaired.record.current_writer, "machine-b");
    assert_eq!(repaired.record.generation, 1);
    assert!(
        !portable
            .path()
            .join("authority-publish/context-mode.json")
            .exists()
    );
}

struct TestRemote<'a> {
    parent: &'a std::sync::Mutex<String>,
}

impl PortableAuthorityPublisher for TestRemote<'_> {
    fn publish(
        &mut self,
        expected_parent: &str,
        _staged_portable_root: &std::path::Path,
    ) -> Result<String, SnapshotError> {
        let mut parent = self.parent.lock().unwrap();
        if parent.as_str() != expected_parent {
            return Err(SnapshotError::StalePortableAuthority);
        }
        *parent = format!("{expected_parent}-next");
        Ok(parent.clone())
    }
}

#[test]
fn two_independent_clones_cannot_both_publish_a_promotion_from_one_remote_parent() {
    let seed = tempfile::tempdir().unwrap();
    let clone_a = tempfile::tempdir().unwrap();
    let clone_b = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([78; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let initial = PortableAuthorityStore::open(seed.path(), &cipher)
        .unwrap()
        .initialize(&database, "machine-a")
        .unwrap();
    copy_tree(seed.path(), clone_a.path());
    copy_tree(seed.path(), clone_b.path());
    let remote_parent = std::sync::Mutex::new("git-parent-1".to_owned());

    let first = PortableAuthorityStore::open(clone_a.path(), &cipher)
        .unwrap()
        .compare_and_swap_writer_published(
            &database,
            &initial.revision,
            "machine-a",
            "machine-b",
            "git-parent-1",
            &mut TestRemote {
                parent: &remote_parent,
            },
        )
        .unwrap();
    assert_eq!(first.authority.record.current_writer, "machine-b");
    assert_eq!(first.repository_revision, "git-parent-1-next");

    assert_eq!(
        PortableAuthorityStore::open(clone_b.path(), &cipher)
            .unwrap()
            .compare_and_swap_writer_published(
                &database,
                &initial.revision,
                "machine-a",
                "machine-c",
                "git-parent-1",
                &mut TestRemote {
                    parent: &remote_parent,
                },
            )
            .unwrap_err(),
        SnapshotError::StalePortableAuthority
    );
    assert_eq!(
        PortableAuthorityStore::open(clone_b.path(), &cipher)
            .unwrap()
            .read(&database)
            .unwrap()
            .record
            .current_writer,
        "machine-a"
    );
}

#[test]
fn fresh_clone_rejects_a_lagging_checkout_against_the_trusted_remote_head() {
    let portable = tempfile::tempdir().unwrap();
    let anchor = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([79; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store =
        PortableAuthorityStore::open_with_trusted_anchor(portable.path(), anchor.path(), &cipher)
            .unwrap();
    store.initialize(&database, "machine-a").unwrap();

    assert_eq!(
        store
            .read_at_repository_revision(&database, "git-old-checkout", "git-current-remote-head",),
        Err(SnapshotError::PortableAuthorityRollback)
    );
}

#[cfg(unix)]
#[test]
fn concrete_git_publisher_allows_only_one_clone_to_advance_the_remote_parent() {
    fn git(directory: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let seed = root.path().join("seed");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "--initial-branch=main"]);
    fs::create_dir(&seed).unwrap();
    git(&seed, &["init", "--initial-branch=main"]);
    git(&seed, &["config", "user.name", "CommonKit Test"]);
    git(&seed, &["config", "user.email", "commonkit-test@localhost"]);
    let cipher = DeterministicTestCipher::new([80; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    PortableAuthorityStore::open(seed.join("kit"), &cipher)
        .unwrap()
        .initialize(&database, "machine-a")
        .unwrap();
    git(&seed, &["add", "kit"]);
    git(&seed, &["commit", "-m", "seed authority"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "main"]);

    let clone_a = root.path().join("clone-a");
    let clone_b = root.path().join("clone-b");
    let remote_text = remote.to_str().unwrap();
    let status_a = std::process::Command::new("/usr/bin/git")
        .args([
            "clone",
            "--branch",
            "main",
            remote_text,
            clone_a.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    let status_b = std::process::Command::new("/usr/bin/git")
        .args([
            "clone",
            "--branch",
            "main",
            remote_text,
            clone_b.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status_a.success() && status_b.success());
    let parent = git(&clone_a, &["rev-parse", "HEAD"]);
    let initial = PortableAuthorityStore::open(clone_a.join("kit"), &cipher)
        .unwrap()
        .read(&database)
        .unwrap();
    let mut publisher_a = ProcessGitAuthorityPublisher::new(
        "/usr/bin/git",
        &clone_a,
        remote_text,
        "main",
        "kit",
        root.path().join("publisher-staging-a"),
    )
    .unwrap();
    PortableAuthorityStore::open(clone_a.join("kit"), &cipher)
        .unwrap()
        .compare_and_swap_writer_published(
            &database,
            &initial.revision,
            "machine-a",
            "machine-b",
            &parent,
            &mut publisher_a,
        )
        .unwrap();

    let mut publisher_b = ProcessGitAuthorityPublisher::new(
        "/usr/bin/git",
        &clone_b,
        remote_text,
        "main",
        "kit",
        root.path().join("publisher-staging-b"),
    )
    .unwrap();
    assert_eq!(
        PortableAuthorityStore::open(clone_b.join("kit"), &cipher)
            .unwrap()
            .compare_and_swap_writer_published(
                &database,
                &initial.revision,
                "machine-a",
                "machine-c",
                &parent,
                &mut publisher_b,
            )
            .unwrap_err(),
        SnapshotError::StalePortableAuthority
    );
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
