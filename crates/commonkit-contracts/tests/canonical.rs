use commonkit_contracts::{canonical_json, digest_json};

#[test]
fn canonical_json_is_key_order_and_newline_independent() {
    let left: serde_json::Value = serde_json::from_str("{\"b\":2,\"a\":1}\n").expect("json");
    let right: serde_json::Value =
        serde_json::from_str("{\r\n  \"a\": 1, \"b\": 2\r\n}").expect("json");

    assert_eq!(
        canonical_json(&left).expect("canonical"),
        br#"{"a":1,"b":2}"#
    );
    assert_eq!(
        canonical_json(&left).expect("canonical"),
        canonical_json(&right).expect("canonical")
    );
    assert_eq!(
        digest_json(&left).expect("digest").as_str(),
        "sha256:43258cff783fe7036d8a43033f830adfc60ec037382473548ac742b888292777"
    );
}
