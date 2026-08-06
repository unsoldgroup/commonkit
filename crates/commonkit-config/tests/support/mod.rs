use commonkit_contracts::{
    GitRevision, LayerDocument, LayerKind, PortableSourcePath, SchemaVersion, Sha256Digest,
    SourceMetadata, StableId,
};

pub fn layer(id: &str, kind: LayerKind, spec: serde_json::Value) -> LayerDocument {
    LayerDocument {
        schema_version: SchemaVersion(1),
        id: StableId::parse(id).expect("id"),
        kind,
        source: SourceMetadata {
            path: PortableSourcePath::parse(format!("layers/{id}.json")).expect("path"),
            revision: Some(
                GitRevision::parse("57a085e7d0b558e71c8d2255b7e60e6c677dee76").expect("revision"),
            ),
            content_digest: Sha256Digest::parse(format!("sha256:{}", "0".repeat(64)))
                .expect("digest"),
        },
        spec,
    }
}
