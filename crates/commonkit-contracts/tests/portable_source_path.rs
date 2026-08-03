use commonkit_contracts::PortableSourcePath;

#[test]
fn the_root_itself_is_a_valid_path() {
    assert_eq!(PortableSourcePath::parse(".").unwrap().as_str(), ".");
}

#[test]
fn nested_paths_remain_valid() {
    for value in ["work", "apps/desktop", "crates/commonkit-cli/src"] {
        assert_eq!(PortableSourcePath::parse(value).unwrap().as_str(), value);
    }
}

#[test]
fn traversal_and_absolute_paths_stay_rejected() {
    for value in [
        "",
        "/etc",
        "..",
        "../escape",
        "work/../..",
        "work/./nested",
        "./work",
        "work/",
        "C:/work",
        "work\\nested",
    ] {
        assert!(
            PortableSourcePath::parse(value).is_err(),
            "{value:?} was accepted"
        );
    }
}
