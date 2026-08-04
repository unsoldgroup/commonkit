use commonkit_contracts::Principal;

#[test]
fn principal_is_a_canonical_plain_string() {
    let principal = Principal::parse("Al-UnsoldGroup").unwrap();
    assert_eq!(principal.as_str(), "al-unsoldgroup");
    assert_eq!(
        serde_json::to_string(&principal).unwrap(),
        r#""al-unsoldgroup""#
    );
    assert_eq!(
        serde_json::from_str::<Principal>(r#""AL-UNSOLDGROUP""#).unwrap(),
        principal
    );

    for invalid in [
        "",
        "-owner",
        "owner-",
        "bad--owner",
        "bad_owner",
        &"a".repeat(40),
    ] {
        assert!(Principal::parse(invalid).is_err(), "{invalid}");
    }
}
