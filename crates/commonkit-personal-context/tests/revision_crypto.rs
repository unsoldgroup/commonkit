use std::{collections::BTreeMap, str::FromStr};

use age::x25519;
use commonkit_contracts::{SchemaVersion, Sha256Digest, StableId};
use commonkit_personal_context::{
    FieldOperation, RevisionBinding, SecretValue, decrypt_revision, encrypt_revision,
    rotate_recipients,
};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn binding() -> RevisionBinding {
    RevisionBinding {
        schema_version: SchemaVersion(1),
        profile_id: StableId::parse("profile-1").unwrap(),
        profile_schema_id: StableId::parse("commonkit-work-profile").unwrap(),
        profile_schema_version: 1,
        revision_id: StableId::parse("revision-1").unwrap(),
        parent_hashes: vec![digest('a')],
    }
}

#[test]
fn revision_values_round_trip_through_either_recovery_recipient() {
    let device = x25519::Identity::generate();
    let offline = x25519::Identity::generate();
    let recipients = vec![device.to_public(), offline.to_public()];
    let fields = BTreeMap::from([
        (
            StableId::parse("communication.tone").unwrap(),
            FieldOperation::Set(SecretValue::new("direct and warm")),
        ),
        (
            StableId::parse("identity.pronouns").unwrap(),
            FieldOperation::Delete,
        ),
    ]);

    let encrypted = encrypt_revision(binding(), fields, &recipients).unwrap();
    let with_device = decrypt_revision(&encrypted, &device).unwrap();
    let with_offline = decrypt_revision(&encrypted, &offline).unwrap();

    assert_eq!(
        with_device[&StableId::parse("communication.tone").unwrap()]
            .expose()
            .unwrap(),
        b"direct and warm"
    );
    assert!(
        with_device[&StableId::parse("identity.pronouns").unwrap()]
            .expose()
            .is_none()
    );
    assert_eq!(with_device, with_offline);
}

#[test]
fn tampering_and_wrong_recipients_are_rejected() {
    let owner = x25519::Identity::generate();
    let stranger = x25519::Identity::generate();
    let fields = BTreeMap::from([(
        StableId::parse("communication-tone").unwrap(),
        FieldOperation::Set(SecretValue::new("concise")),
    )]);
    let mut encrypted = encrypt_revision(binding(), fields, &[owner.to_public()]).unwrap();

    assert!(decrypt_revision(&encrypted, &stranger).is_err());
    encrypted.fields[0]
        .ciphertext
        .as_mut()
        .unwrap()
        .replace_range(0..1, "A");
    assert!(decrypt_revision(&encrypted, &owner).is_err());

    let fields = BTreeMap::from([(
        StableId::parse("communication-tone").unwrap(),
        FieldOperation::Set(SecretValue::new("concise")),
    )]);
    let mut recipient_tamper = encrypt_revision(binding(), fields, &[owner.to_public()]).unwrap();
    recipient_tamper.wrapped_data_keys[0].recipient = stranger.to_public().to_string();
    assert!(decrypt_revision(&recipient_tamper, &owner).is_err());
}

#[test]
fn recipient_text_is_interoperable_age_x25519() {
    let identity = x25519::Identity::generate();
    let encoded = identity.to_public().to_string();
    assert!(x25519::Recipient::from_str(&encoded).is_ok());
}

#[test]
fn portable_document_and_debug_output_never_expose_plaintext() {
    let identity = x25519::Identity::generate();
    let secret = SecretValue::new("never-store-this-phrase");
    assert_eq!(format!("{secret:?}"), "SecretValue([REDACTED])");
    let fields = BTreeMap::from([(
        StableId::parse("agents-response-style").unwrap(),
        FieldOperation::Set(secret),
    )]);

    let encrypted = encrypt_revision(binding(), fields, &[identity.to_public()]).unwrap();
    let portable = serde_json::to_string(&encrypted).unwrap();
    assert!(!portable.contains("never-store-this-phrase"));
    assert!(!portable.contains("REDACTED"));
}

#[test]
fn rotating_recipients_reencrypts_fields_and_revokes_the_old_identity() {
    let old = x25519::Identity::generate();
    let replacement = x25519::Identity::generate();
    let fields = BTreeMap::from([(
        StableId::parse("communication-tone").unwrap(),
        FieldOperation::Set(SecretValue::new("plainspoken")),
    )]);
    let encrypted = encrypt_revision(binding(), fields, &[old.to_public()]).unwrap();

    let rotated = rotate_recipients(&encrypted, &old, &[replacement.to_public()]).unwrap();
    assert!(decrypt_revision(&rotated, &old).is_err());
    assert_eq!(
        decrypt_revision(&rotated, &replacement).unwrap()
            [&StableId::parse("communication-tone").unwrap()]
            .expose()
            .unwrap(),
        b"plainspoken"
    );
    assert_ne!(encrypted.recipient_set_digest, rotated.recipient_set_digest);
}
