use commonkit_contracts::{SchemaVersion, StableId};
use commonkit_personal_context::{
    DeletionDisposition, PersonalContextDeletionState, PersonalContextLease,
};

#[test]
fn observed_tombstone_clears_controlled_state_and_is_replay_safe() {
    let mut state = PersonalContextDeletionState {
        schema_version: SchemaVersion(1),
        profile_id: StableId::parse("profile-1").unwrap(),
        latest_generation: 4,
        tombstone_generation: None,
        active_ciphertext_objects: 3,
        controlled_cache_entries: 2,
        active_grants: 1,
    };

    let first = state.observe_tombstone(5).unwrap();
    assert_eq!(first.disposition, DeletionDisposition::Applied);
    assert_eq!(first.removed_ciphertext_objects, 3);
    assert_eq!(first.removed_cache_entries, 2);
    assert_eq!(state.active_ciphertext_objects, 0);
    assert_eq!(state.controlled_cache_entries, 0);
    assert_eq!(state.active_grants, 0);

    let replay = state.observe_tombstone(5).unwrap();
    assert_eq!(replay.disposition, DeletionDisposition::AlreadyApplied);
    assert!(!state.accepts_revision("profile-1", 5));
    assert!(!state.accepts_revision("profile-1", 6));
    assert!(state.accepts_revision("profile-2", 1));
}

#[test]
fn offline_decryption_stops_when_the_authorization_lease_expires() {
    let lease = PersonalContextLease {
        issued_at_unix_ms: 100,
        expires_at_unix_ms: 200,
        revocation_generation: 7,
    };
    assert!(lease.authorizes(199, 7));
    assert!(!lease.authorizes(200, 7));
    assert!(!lease.authorizes(150, 8));
}
