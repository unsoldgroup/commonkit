use commonkit_about_me::{ClaimCategory, ClaimInput, ProfileStore, ScopedView, SuggestionDecision};

fn view(project: &str) -> ScopedView {
    ScopedView {
        loadout_id: "personal".into(),
        project_id: project.into(),
    }
}

#[test]
fn approved_claims_are_searchable_only_in_their_scoped_view() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("about-me.sqlite");
    let mut store = ProfileStore::open(&database, b"correct horse battery staple").unwrap();

    let draft = store
        .create_draft(
            0,
            "Call me Al.",
            vec![ClaimInput {
                topic_key: "communication/answer-style".into(),
                category: ClaimCategory::Communication,
                text: "I prefer concise answers with plain language.".into(),
                views: vec![view("commonkit")],
            }],
        )
        .unwrap();
    let published = store.publish_draft(&draft.id, 0).unwrap();

    assert_eq!(published.revision, 1);
    assert_eq!(
        store.summary(&view("commonkit")).unwrap().as_deref(),
        Some("Call me Al.")
    );
    assert_eq!(
        store
            .search(&view("commonkit"), "plain language", 10)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search(&view("another-project"), "plain language", 10)
            .unwrap()
            .is_empty()
    );

    let bytes = std::fs::read(database).unwrap();
    assert!(
        !bytes
            .windows(b"plain language".len())
            .any(|window| window == b"plain language")
    );
}

#[cfg(unix)]
#[test]
fn profile_database_is_created_private_and_symlinks_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("profile.sqlite");
    ProfileStore::open(&database, b"owner-key").unwrap();
    assert_eq!(
        std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let linked = root.path().join("linked.sqlite");
    symlink(&database, &linked).unwrap();
    assert!(ProfileStore::open(&linked, b"owner-key").is_err());
}

#[test]
fn a_confirmed_contradiction_supersedes_the_old_claim_without_a_second_review() {
    let directory = tempfile::tempdir().unwrap();
    let mut store =
        ProfileStore::open(directory.path().join("about-me.sqlite"), b"profile-key").unwrap();
    let first = store
        .create_draft(
            0,
            "",
            vec![ClaimInput {
                topic_key: "communication/answer-style".into(),
                category: ClaimCategory::Communication,
                text: "I prefer detailed answers.".into(),
                views: vec![view("commonkit")],
            }],
        )
        .unwrap();
    let first = store.publish_draft(&first.id, 0).unwrap();
    let active = store
        .search(&view("commonkit"), "detailed", 10)
        .unwrap()
        .pop()
        .unwrap();

    let replacement = store
        .resolve_contradiction(
            &active.claim_id,
            first.revision,
            "I prefer concise answers.",
            "I now prefer concise answers.",
        )
        .unwrap();

    assert_eq!(replacement.revision, 2);
    assert!(
        store
            .search(&view("commonkit"), "detailed", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .search(&view("commonkit"), "concise", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rejected_suggestions_purge_content_but_prevent_repeat_proposals() {
    let directory = tempfile::tempdir().unwrap();
    let mut store =
        ProfileStore::open(directory.path().join("about-me.sqlite"), b"profile-key").unwrap();
    let suggestion = store
        .suggest(
            view("commonkit"),
            ClaimInput {
                topic_key: "workflow/testing".into(),
                category: ClaimCategory::Workflow,
                text: "I prefer test-first changes.".into(),
                views: vec![view("commonkit")],
            },
            "Please use test-first changes.",
            "codex",
        )
        .unwrap();

    store
        .decide_suggestion(&suggestion.id, SuggestionDecision::Reject)
        .unwrap();

    assert!(store.pending_suggestions().unwrap().is_empty());
    assert!(
        store
            .suggest(
                view("commonkit"),
                ClaimInput {
                    topic_key: "workflow/testing".into(),
                    category: ClaimCategory::Workflow,
                    text: "I prefer test-first changes.".into(),
                    views: vec![view("commonkit")],
                },
                "Please use test-first changes.",
                "claude",
            )
            .is_err()
    );
}
