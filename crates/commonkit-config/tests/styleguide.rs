use std::collections::{BTreeMap, BTreeSet};

use commonkit_config::{StyleguidePolicy, StyleguideResolutionError, resolve_styleguide_selection};
use commonkit_contracts::{
    GitRevision, SchemaVersion, Sha256Digest, StableId, StyleguideActivation, StyleguideDescriptor,
    StyleguidePackageProvenance, StyleguideSelection,
};

fn id(value: &str) -> StableId {
    StableId::parse(value).unwrap()
}

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn descriptor() -> StyleguideDescriptor {
    StyleguideDescriptor {
        schema_version: SchemaVersion(1),
        id: id("commonkit-technical-writing"),
        exported_skill: id("technical-writing"),
        modes: BTreeSet::from([id("strict"), id("technical")]),
        prose_scopes: BTreeSet::from([id("readme"), id("procedure")]),
        exclusions: BTreeSet::from([id("source-code"), id("marketing-copy")]),
        evaluation_suite: id("technical-writing-v1"),
        evaluation_suite_digest: digest('a'),
        upstream_url: "https://github.com/woosal1337/blog".into(),
        upstream_revision: GitRevision::parse("b912d5fa59f368253683af2ebfac64ad6d08312d").unwrap(),
        retention_map_digest: digest('b'),
        package: StyleguidePackageProvenance {
            id: id("commonkit-styleguide"),
            version: "0.1.0".into(),
            manifest_digest: digest('c'),
            lock_digest: digest('d'),
        },
    }
}

#[test]
fn resolves_one_routed_export_and_binds_descriptor_provenance() {
    let selection = StyleguideSelection {
        skill_id: id("technical-writing"),
        activation: StyleguideActivation::Routed,
    };
    let descriptors = BTreeMap::from([(id("technical-writing"), descriptor())]);

    let resolved = resolve_styleguide_selection(
        Some(&selection),
        &descriptors,
        &BTreeSet::from([id("technical-writing")]),
        &StyleguidePolicy::default(),
    )
    .expect("resolve")
    .expect("selected");

    assert_eq!(resolved.selection, selection);
    assert_eq!(
        resolved.binding.descriptor_digest,
        descriptor().digest().unwrap()
    );
    assert_eq!(resolved.binding.package_lock_digest, digest('d'));
    assert_eq!(resolved.binding.evaluation_suite_digest, digest('a'));
    assert_eq!(resolved.binding.retention_map_digest, digest('b'));
    assert_eq!(
        resolved.binding.digest().unwrap(),
        resolved.binding.digest().unwrap()
    );
}

#[test]
fn rejects_missing_ambiguous_denied_and_unpinned_exports() {
    let selection = StyleguideSelection {
        skill_id: id("technical-writing"),
        activation: StyleguideActivation::Routed,
    };
    let descriptors = BTreeMap::from([(id("technical-writing"), descriptor())]);

    assert_eq!(
        resolve_styleguide_selection(
            Some(&selection),
            &descriptors,
            &BTreeSet::new(),
            &StyleguidePolicy::default(),
        ),
        Err(StyleguideResolutionError::MissingExport(id(
            "technical-writing"
        )))
    );
    assert_eq!(
        resolve_styleguide_selection(
            Some(&selection),
            &descriptors,
            &BTreeSet::from([id("technical-writing"), id("other-writing")]),
            &StyleguidePolicy::default(),
        ),
        Err(StyleguideResolutionError::AmbiguousExports)
    );
    assert_eq!(
        resolve_styleguide_selection(
            Some(&selection),
            &descriptors,
            &BTreeSet::from([id("technical-writing")]),
            &StyleguidePolicy {
                denied: true,
                pinned_skill_id: None,
            },
        ),
        Err(StyleguideResolutionError::DeniedByOrganization)
    );
    assert_eq!(
        resolve_styleguide_selection(
            Some(&selection),
            &descriptors,
            &BTreeSet::from([id("technical-writing")]),
            &StyleguidePolicy {
                denied: false,
                pinned_skill_id: Some(id("other-writing")),
            },
        ),
        Err(StyleguideResolutionError::OrganizationPinMismatch {
            expected: id("other-writing"),
            actual: id("technical-writing"),
        })
    );
}

#[test]
fn rejects_a_descriptor_that_claims_a_different_export() {
    let selection = StyleguideSelection {
        skill_id: id("technical-writing"),
        activation: StyleguideActivation::Routed,
    };
    let mut mismatched = descriptor();
    mismatched.exported_skill = id("other-writing");

    assert_eq!(
        resolve_styleguide_selection(
            Some(&selection),
            &BTreeMap::from([(id("technical-writing"), mismatched)]),
            &BTreeSet::from([id("technical-writing")]),
            &StyleguidePolicy::default(),
        ),
        Err(StyleguideResolutionError::DescriptorExportMismatch {
            selection: id("technical-writing"),
            descriptor: id("other-writing"),
        })
    );
}
