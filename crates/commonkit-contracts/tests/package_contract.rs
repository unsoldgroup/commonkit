use std::collections::BTreeSet;

use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, RustToolchainProfile, StableId,
};

fn id(value: &str) -> StableId {
    StableId::parse(value).unwrap()
}

#[test]
fn package_declaration_requires_an_exact_manager_matched_selector() {
    let mismatched = PackageDeclaration {
        id: id("ripgrep"),
        version: "14.1.1".into(),
        manager: PackageManager::Homebrew,
        source: id("homebrew-core"),
        selector: Some(PackageSelector::AptBinary {
            name: "ripgrep".into(),
            architecture: None,
        }),
    };
    assert!(mismatched.validate().is_err());

    let floating = PackageDeclaration {
        id: id("node"),
        version: "latest".into(),
        manager: PackageManager::Nvm,
        source: id("nodejs-official"),
        selector: Some(PackageSelector::NodeRuntime {}),
    };
    assert!(floating.validate().is_err());

    let exact = PackageDeclaration {
        id: id("rust"),
        version: "1.85.1".into(),
        manager: PackageManager::Rustup,
        source: id("rustup-official"),
        selector: Some(PackageSelector::RustToolchain {
            profile: RustToolchainProfile::Minimal,
            components: BTreeSet::from(["rustfmt".into()]),
            targets: BTreeSet::from(["aarch64-apple-darwin".into()]),
        }),
    };
    exact.validate().unwrap();
}

#[test]
fn package_selector_wire_vocabulary_denies_unknown_types_and_fields() {
    let unknown = serde_json::json!({ "type": "cargo_crate", "name": "ripgrep" });
    assert!(serde_json::from_value::<PackageSelector>(unknown).is_err());

    let extra = serde_json::json!({
        "type": "node_runtime",
        "providerResolution": "latest"
    });
    assert!(serde_json::from_value::<PackageSelector>(extra).is_err());
}
