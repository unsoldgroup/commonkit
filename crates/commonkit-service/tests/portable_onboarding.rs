use commonkit_contracts::portable_context::{
    GitHubOnboardingAttempt, GitHubOnboardingState, RepositoryRole,
};
use commonkit_contracts::{SchemaVersion, Sha256Digest, StableId};
use commonkit_service::OnboardingStateStore;

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn attempt() -> GitHubOnboardingAttempt {
    GitHubOnboardingAttempt {
        schema_version: SchemaVersion(1),
        id: StableId::parse("attempt-1").unwrap(),
        repository_role: RepositoryRole::PersonalContext,
        account_node_id: "U_user".into(),
        repository_node_id: None,
        plan_digest: digest('a'),
        confirmation_digest: digest('b'),
        idempotency_key: StableId::parse("request-1").unwrap(),
        registration_id: None,
        state: GitHubOnboardingState::Planned,
        failure_code: None,
        updated_at_unix_ms: 100,
    }
}

#[test]
fn remote_creation_survives_restart_as_a_user_owned_orphan() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("onboarding.json");
    let store = OnboardingStateStore::open(&path).unwrap();
    store.begin(attempt()).unwrap();
    store
        .mark_remote_created("attempt-1", "R_repo", 110)
        .unwrap();
    store
        .mark_orphaned(
            "attempt-1",
            StableId::parse("registration-failed").unwrap(),
            120,
        )
        .unwrap();

    let reopened = OnboardingStateStore::open(&path).unwrap();
    let orphan = reopened.get("attempt-1").unwrap().unwrap();
    assert_eq!(orphan.state, GitHubOnboardingState::Orphaned);
    assert_eq!(orphan.repository_node_id.as_deref(), Some("R_repo"));
    assert!(orphan.registration_id.is_none());
}

#[test]
fn completion_is_idempotent_but_cannot_substitute_a_registration() {
    let temporary = tempfile::tempdir().unwrap();
    let store = OnboardingStateStore::open(temporary.path().join("onboarding.json")).unwrap();
    store.begin(attempt()).unwrap();
    store
        .mark_remote_created("attempt-1", "R_repo", 110)
        .unwrap();
    let registration = StableId::parse("registration-1").unwrap();
    store.mark_registering("attempt-1", 115).unwrap();
    store
        .mark_registered("attempt-1", registration.clone(), 120)
        .unwrap();
    store.mark_verifying("attempt-1", 125).unwrap();
    store
        .complete("attempt-1", registration.clone(), 130)
        .unwrap();
    store.complete("attempt-1", registration, 140).unwrap();
    assert!(
        store
            .complete("attempt-1", StableId::parse("registration-2").unwrap(), 150,)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn refuses_to_read_onboarding_state_through_a_symbolic_link() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target.json");
    std::fs::write(&target, r#"{"attempts":{}}"#).unwrap();
    let link = temporary.path().join("onboarding.json");
    symlink(&target, &link).unwrap();

    assert!(OnboardingStateStore::open(link).is_err());
}
