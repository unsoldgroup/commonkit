use commonkit_config::{LayerSet, LayerSetError};
use commonkit_contracts::{
    LayerDocument, LayerKind, PortableSourcePath, SchemaVersion, Sha256Digest, SourceMetadata,
    StableId,
};

fn layer(id: &str, kind: LayerKind) -> LayerDocument {
    LayerDocument {
        schema_version: SchemaVersion(1),
        id: StableId::parse(id).expect("id"),
        kind,
        source: SourceMetadata {
            path: PortableSourcePath::parse(format!("layers/{id}.json")).expect("path"),
            revision: None,
            content_digest: Sha256Digest::parse(format!("sha256:{}", "0".repeat(64)))
                .expect("digest"),
        },
        spec: serde_json::json!({}),
    }
}

#[test]
fn orders_layers_by_the_fixed_product_precedence() {
    let set = LayerSet::new(vec![
        layer("target", LayerKind::TargetOverrides),
        layer("org", LayerKind::OrganizationPolicy),
        layer("base", LayerKind::PublicBase),
    ])
    .expect("valid layers");

    assert_eq!(
        set.iter().map(|layer| layer.kind).collect::<Vec<_>>(),
        vec![
            LayerKind::PublicBase,
            LayerKind::OrganizationPolicy,
            LayerKind::TargetOverrides,
        ]
    );
}

#[test]
fn requires_base_and_organization_and_rejects_duplicates() {
    assert!(matches!(
        LayerSet::new(vec![layer("base", LayerKind::PublicBase)]),
        Err(LayerSetError::MissingRequired(
            LayerKind::OrganizationPolicy
        ))
    ));
    assert!(matches!(
        LayerSet::new(vec![
            layer("base", LayerKind::PublicBase),
            layer("org-a", LayerKind::OrganizationPolicy),
            layer("org-b", LayerKind::OrganizationPolicy),
        ]),
        Err(LayerSetError::DuplicateKind(LayerKind::OrganizationPolicy))
    ));
}

#[test]
fn validates_layer_specs_before_applying_precedence() {
    let mut base = layer("base", LayerKind::PublicBase);
    base.spec = serde_json::json!({"theme": "dark"});

    assert!(matches!(
        LayerSet::new(vec![base, layer("org", LayerKind::OrganizationPolicy)]),
        Err(LayerSetError::UnsupportedLayerSpecField { field }) if field == "theme"
    ));
}
