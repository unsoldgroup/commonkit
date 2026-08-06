use commonkit_contracts::{SchemaVersion, Sha256Digest, StableId};
use commonkit_personal_context::{
    CryptoError, EncryptedRevision, EncryptedRevisionStore, RevisionBinding,
};
use std::sync::Arc;

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn revision() -> EncryptedRevision {
    EncryptedRevision {
        schema_version: SchemaVersion(1),
        binding: RevisionBinding {
            schema_version: SchemaVersion(1),
            profile_id: StableId::parse("profile-1").unwrap(),
            profile_schema_id: StableId::parse("commonkit-work-profile").unwrap(),
            profile_schema_version: 1,
            revision_id: StableId::parse("revision-1").unwrap(),
            parent_hashes: vec![],
        },
        recipient_set_digest: digest('a'),
        wrapped_data_keys: vec![],
        fields: vec![],
    }
}

#[test]
fn encrypted_revision_staging_is_private_idempotent_and_substitution_safe() {
    let temporary = tempfile::tempdir().unwrap();
    let store = EncryptedRevisionStore::open(temporary.path().join("staged")).unwrap();
    let revision = revision();
    let first = store.stage(&revision).unwrap();
    assert_eq!(store.stage(&revision).unwrap(), first);
    let path = temporary.path().join("staged/revision-1.json");
    assert!(path.is_file());
    assert!(!std::fs::read_to_string(path).unwrap().contains("plaintext"));

    let changed = EncryptedRevision {
        recipient_set_digest: digest('b'),
        ..revision
    };
    assert!(matches!(
        store.stage(&changed),
        Err(CryptoError::RevisionCollision)
    ));
}

#[test]
fn encrypted_revision_staging_rejects_relative_roots() {
    assert!(matches!(
        EncryptedRevisionStore::open("relative/staged"),
        Err(CryptoError::UnsafeStore)
    ));
}

#[test]
fn concurrent_idempotent_staging_does_not_share_temporary_files() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Arc::new(EncryptedRevisionStore::open(temporary.path().join("staged")).unwrap());
    let revision = Arc::new(revision());
    let workers = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let revision = Arc::clone(&revision);
            std::thread::spawn(move || store.stage(&revision))
        })
        .collect::<Vec<_>>();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap().unwrap())
        .collect::<Vec<_>>();
    assert!(results.iter().all(|digest| digest == &results[0]));
    assert_eq!(
        std::fs::read_dir(temporary.path().join("staged"))
            .unwrap()
            .count(),
        1
    );
}
