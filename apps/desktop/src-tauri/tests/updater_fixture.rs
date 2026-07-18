use base64::Engine;
use minisign_verify::{PublicKey, Signature};

const PUBLIC_KEY: &str = include_str!("fixtures/updater/public.key");
const V1_PAYLOAD: &[u8] = include_bytes!("fixtures/updater/0.1.0.payload");
const V1_SIGNATURE: &str = include_str!("fixtures/updater/0.1.0.payload.sig");
const V2_PAYLOAD: &[u8] = include_bytes!("fixtures/updater/0.2.0.payload");
const V2_SIGNATURE: &str = include_str!("fixtures/updater/0.2.0.payload.sig");

fn decode_signature(encoded: &str) -> Signature {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .expect("fixture signature is base64");
    Signature::decode(std::str::from_utf8(&decoded).expect("fixture signature is UTF-8"))
        .expect("fixture signature is minisign")
}

#[test]
fn two_version_fixture_is_signed_by_the_checked_in_public_key() {
    let key = PublicKey::decode(PUBLIC_KEY).expect("fixture public key is valid");
    key.verify(V1_PAYLOAD, &decode_signature(V1_SIGNATURE), true)
        .expect("0.1.0 fixture signature is valid");
    key.verify(V2_PAYLOAD, &decode_signature(V2_SIGNATURE), true)
        .expect("0.2.0 fixture signature is valid");
    assert_ne!(V1_PAYLOAD, V2_PAYLOAD);
}

#[test]
fn invalid_or_replayed_signature_is_rejected() {
    let key = PublicKey::decode(PUBLIC_KEY).expect("fixture public key is valid");
    let v2_signature = decode_signature(V2_SIGNATURE);
    assert!(key.verify(b"tampered update", &v2_signature, true).is_err());
    assert!(key.verify(V1_PAYLOAD, &v2_signature, true).is_err());
}
