use commonkit_adapters::{
    ContentReference, ContentSensitivity, FileMode, FilesystemIntent, NormalizedManagedPath,
    NormalizedResource, OwnershipError, OwnershipRules, ResourceProvenance, SafeSymlinkTarget,
    SymlinkTargetKind, materialized_resources_digest, validate_ownership,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn provenance(provider: &str, source: &str) -> ResourceProvenance {
    ResourceProvenance {
        provider_id: StableId::parse(provider).expect("provider"),
        provider_version: "1.2.3".into(),
        input_digest: digest('a'),
        source: source.into(),
    }
}

fn file(provider: &str, path: &str) -> NormalizedResource {
    NormalizedResource {
        intent: FilesystemIntent::File {
            path: NormalizedManagedPath::parse(path).expect("path"),
            content: ContentReference {
                digest: digest('f'),
                bytes: 7,
                sensitivity: ContentSensitivity::Portable,
            },
            mode: Some(FileMode::parse(0o600).expect("mode")),
            expected_before: None,
        },
        provenance: provenance(provider, path),
    }
}

fn directory(provider: &str, path: &str, exact: bool) -> NormalizedResource {
    NormalizedResource {
        intent: FilesystemIntent::Directory {
            path: NormalizedManagedPath::parse(path).expect("path"),
            mode: None,
            exact,
        },
        provenance: provenance(provider, path),
    }
}

fn directory_with_mode(provider: &str, path: &str, exact: bool, mode: u32) -> NormalizedResource {
    let mut resource = directory(provider, path, exact);
    if let FilesystemIntent::Directory {
        mode: resource_mode,
        ..
    } = &mut resource.intent
    {
        *resource_mode = Some(FileMode::parse(mode).unwrap());
    }
    resource
}

fn removal(provider: &str, path: &str) -> NormalizedResource {
    NormalizedResource {
        intent: FilesystemIntent::Remove {
            path: NormalizedManagedPath::parse(path).expect("path"),
            expected_before: None,
        },
        provenance: provenance(provider, path),
    }
}

fn rules(case_sensitive: bool) -> OwnershipRules {
    OwnershipRules::new(
        case_sensitive,
        vec![NormalizedManagedPath::parse("home").expect("root")],
        vec![
            NormalizedManagedPath::parse("home/.commonkit").expect("protected"),
            NormalizedManagedPath::parse("home/.ssh/control").expect("protected"),
        ],
    )
    .expect("rules")
}

#[test]
fn portable_paths_and_symlink_targets_are_lexically_safe() {
    for path in [
        "",
        "/etc/passwd",
        "../escape",
        "a/../escape",
        "C:/secret",
        "a\\b",
    ] {
        assert!(
            NormalizedManagedPath::parse(path).is_err(),
            "accepted {path}"
        );
    }
    let link = NormalizedManagedPath::parse("home/bin/tool").expect("link");
    assert_eq!(
        SafeSymlinkTarget::parse(&link, "../scripts/tool")
            .expect("contained target")
            .as_str(),
        "../scripts/tool"
    );
    for target in ["/usr/bin/tool", "../../../escape", "C:/tool", "a\\b"] {
        assert!(
            SafeSymlinkTarget::parse(&link, target).is_err(),
            "accepted {target}"
        );
    }
}

#[test]
fn ownership_revalidates_deserialized_symlink_containment() {
    let resource: NormalizedResource = serde_json::from_value(serde_json::json!({
        "intent": {
            "type": "symlink",
            "path": "home/bin/tool",
            "target": "../../../escape",
            "expected_before": null
        },
        "provenance": {
            "providerId": "chezmoi",
            "providerVersion": "1.2.3",
            "inputDigest": digest('a'),
            "source": "dot_symlink"
        }
    }))
    .expect("wire form is decoded before path-relative validation");

    assert!(matches!(
        validate_ownership(&[resource], &rules(true)),
        Err(OwnershipError::UnsafeSymlinkTarget { .. })
    ));
}

#[test]
fn rejects_duplicate_and_case_folded_path_claims() {
    let duplicate = validate_ownership(
        &[
            file("native", "home/config"),
            file("chezmoi", "home/config"),
        ],
        &rules(true),
    )
    .expect_err("duplicate");
    assert!(matches!(duplicate, OwnershipError::DuplicatePath { .. }));

    validate_ownership(
        &[
            file("native", "home/Config"),
            file("chezmoi", "home/config"),
        ],
        &rules(true),
    )
    .expect("case-sensitive target");
    let folded = validate_ownership(
        &[
            file("native", "home/Config"),
            file("chezmoi", "home/config"),
        ],
        &rules(false),
    )
    .expect_err("case-folded collision");
    assert!(matches!(folded, OwnershipError::CaseCollision { .. }));
}

#[test]
fn rejects_exact_directory_ancestry_and_type_or_removal_conflicts() {
    let exact = validate_ownership(
        &[
            directory("chezmoi", "home/config", true),
            file("native", "home/config/tool.toml"),
        ],
        &rules(true),
    )
    .expect_err("exact directory ancestry");
    assert!(matches!(
        exact,
        OwnershipError::ExactDirectoryConflict { .. }
    ));

    let type_conflict = validate_ownership(
        &[
            directory("native", "home/cache", false),
            file("chezmoi", "home/cache"),
        ],
        &rules(true),
    )
    .expect_err("type conflict");
    assert!(matches!(type_conflict, OwnershipError::TypeConflict { .. }));

    let removal_conflict = validate_ownership(
        &[
            removal("native", "home/obsolete"),
            file("chezmoi", "home/obsolete"),
        ],
        &rules(true),
    )
    .expect_err("removal conflict");
    assert!(matches!(
        removal_conflict,
        OwnershipError::RemovalConflict { .. }
    ));
}

#[test]
fn rejects_claims_outside_roots_and_below_protected_state() {
    assert!(matches!(
        validate_ownership(&[file("native", "other/config")], &rules(true)),
        Err(OwnershipError::OutsideDeclaredRoot { .. })
    ));
    assert!(matches!(
        validate_ownership(
            &[file("native", "home/.commonkit/receipts/1")],
            &rules(true)
        ),
        Err(OwnershipError::ProtectedPath { .. })
    ));
}

#[test]
fn destructive_ancestor_claims_cannot_enclose_protected_state() {
    for resource in [
        removal("native", "home"),
        directory("native", "home/.ssh", true),
    ] {
        assert!(matches!(
            validate_ownership(&[resource], &rules(true)),
            Err(OwnershipError::ProtectedPath { .. })
        ));
    }

    validate_ownership(&[directory("native", "home/.ssh", false)], &rules(true))
        .expect("a non-exact ancestor cannot delete protected descendants");
    assert!(matches!(
        validate_ownership(
            &[directory_with_mode("native", "home/.ssh", false, 0o700)],
            &rules(true)
        ),
        Err(OwnershipError::ProtectedPath { .. })
    ));
}

#[test]
fn symlink_targets_cannot_resolve_into_protected_state() {
    let link = NormalizedManagedPath::parse("home/bin/commonkit-state").unwrap();
    let resource = NormalizedResource {
        intent: FilesystemIntent::Symlink {
            path: link.clone(),
            target: SafeSymlinkTarget::parse(&link, "../.commonkit").unwrap(),
            target_kind: SymlinkTargetKind::Directory,
            expected_before: None,
        },
        provenance: provenance("native", "protected-link"),
    };

    assert!(matches!(
        validate_ownership(&[resource], &rules(true)),
        Err(OwnershipError::ProtectedPath { .. })
    ));
}

#[test]
fn case_insensitive_targets_cannot_alias_declared_or_protected_roots() {
    assert!(matches!(
        validate_ownership(
            &[file("native", "HOME/.COMMONKIT/receipts/1")],
            &rules(false)
        ),
        Err(OwnershipError::ProtectedPath { .. })
    ));
    assert!(matches!(
        validate_ownership(&[file("native", "OTHER/config")], &rules(false)),
        Err(OwnershipError::OutsideDeclaredRoot { .. })
    ));
}

#[test]
fn legacy_file_symlink_intents_keep_their_wire_shape() {
    let path = NormalizedManagedPath::parse("home/bin/tool").unwrap();
    let file_target = FilesystemIntent::Symlink {
        path: path.clone(),
        target: SafeSymlinkTarget::parse(&path, "../lib/tool").unwrap(),
        target_kind: SymlinkTargetKind::File,
        expected_before: None,
    };
    let directory_target = FilesystemIntent::Symlink {
        path,
        target: SafeSymlinkTarget::parse(
            &NormalizedManagedPath::parse("home/bin/tool").unwrap(),
            "../lib/tool",
        )
        .unwrap(),
        target_kind: SymlinkTargetKind::Directory,
        expected_before: None,
    };

    assert!(
        serde_json::to_value(file_target).unwrap()["target_kind"].is_null(),
        "the default file kind must not change legacy payload bytes"
    );
    assert_eq!(
        serde_json::to_value(directory_target).unwrap()["target_kind"],
        "directory"
    );
}

#[test]
fn canonical_materialization_digest_is_order_independent_and_semantic() {
    let first = file("native", "home/a");
    let second = directory("chezmoi", "home/b", true);
    let forward = materialized_resources_digest(&[first.clone(), second.clone()]).expect("digest");
    let reverse = materialized_resources_digest(&[second.clone(), first.clone()]).expect("digest");
    assert_eq!(forward, reverse);

    let mut changed = second;
    changed.intent = FilesystemIntent::Directory {
        path: NormalizedManagedPath::parse("home/b").expect("path"),
        mode: None,
        exact: false,
    };
    assert_ne!(
        forward,
        materialized_resources_digest(&[first, changed]).expect("changed digest")
    );
}
