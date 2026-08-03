use std::collections::BTreeMap;

use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_personal_context::{EncryptedField, EncryptedOperation, merge_concurrent_operations};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn field(name: &str, marker: char) -> EncryptedField {
    EncryptedField {
        field_id: StableId::parse(name).unwrap(),
        operation: EncryptedOperation::Set,
        nonce: Some("bm9uY2U=".into()),
        ciphertext: Some(marker.to_string()),
        binding_digest: digest(marker),
    }
}

#[test]
fn different_fields_merge_and_same_field_changes_remain_conflicts() {
    let left = BTreeMap::from([
        (StableId::parse("tone").unwrap(), field("tone", 'a')),
        (StableId::parse("role").unwrap(), field("role", 'b')),
    ]);
    let right = BTreeMap::from([
        (StableId::parse("channels").unwrap(), field("channels", 'c')),
        (StableId::parse("tone").unwrap(), field("tone", 'd')),
    ]);

    let outcome = merge_concurrent_operations(left, right);
    assert_eq!(outcome.merged.len(), 2);
    assert!(
        outcome
            .merged
            .contains_key(&StableId::parse("role").unwrap())
    );
    assert!(
        outcome
            .merged
            .contains_key(&StableId::parse("channels").unwrap())
    );
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(
        outcome.conflicts[0].field_id,
        StableId::parse("tone").unwrap()
    );
}
