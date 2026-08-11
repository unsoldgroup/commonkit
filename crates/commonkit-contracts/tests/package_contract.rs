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

#[test]
fn controller_resolution_requires_typed_manager_specific_exact_versions() {
    let legacy: PackageDeclaration = serde_json::from_value(serde_json::json!({
        "id": "ripgrep",
        "version": "14.1.1",
        "manager": "homebrew",
        "source": "homebrew-core"
    }))
    .expect("legacy declaration remains decodable");
    legacy.validate().expect("legacy compatibility validation");
    assert!(legacy.validate_for_resolution().is_err());

    for (manager, selector, version) in [
        (
            PackageManager::Homebrew,
            PackageSelector::HomebrewFormula {
                name: "ripgrep".into(),
            },
            "14",
        ),
        (
            PackageManager::Apt,
            PackageSelector::AptBinary {
                name: "ripgrep".into(),
                architecture: None,
            },
            "2",
        ),
        (PackageManager::Nvm, PackageSelector::NodeRuntime {}, "22"),
        (PackageManager::Fnm, PackageSelector::NodeRuntime {}, "22.x"),
        (
            PackageManager::Rustup,
            PackageSelector::RustToolchain {
                profile: RustToolchainProfile::Minimal,
                components: BTreeSet::new(),
                targets: BTreeSet::new(),
            },
            "nightly",
        ),
        (
            PackageManager::Rustup,
            PackageSelector::RustToolchain {
                profile: RustToolchainProfile::Minimal,
                components: BTreeSet::new(),
                targets: BTreeSet::new(),
            },
            "main",
        ),
    ] {
        let declaration = PackageDeclaration {
            id: id("runtime"),
            version: version.into(),
            manager,
            source: id(match manager {
                PackageManager::Homebrew => "homebrew-core",
                PackageManager::Apt => "ubuntu-main",
                PackageManager::Nvm => "nodejs-nvm",
                PackageManager::Fnm => "nodejs-fnm",
                PackageManager::Rustup => "rustup-official",
            }),
            selector: Some(selector),
        };
        assert!(
            declaration.validate_for_resolution().is_err(),
            "{manager:?} accepted floating version {version}"
        );
    }

    let exact_node = PackageDeclaration {
        id: id("node"),
        version: "22.14.0".into(),
        manager: PackageManager::Fnm,
        source: id("nodejs-fnm"),
        selector: Some(PackageSelector::NodeRuntime {}),
    };
    exact_node.validate_for_resolution().unwrap();
}
