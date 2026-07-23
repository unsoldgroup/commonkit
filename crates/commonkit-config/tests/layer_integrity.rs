mod support;

use commonkit_config::{layer_content_digest, validate_layer_content_digest};
use commonkit_contracts::LayerKind;
use support::layer;

#[test]
fn layer_digest_binds_canonical_content_without_hashing_itself() {
    let mut document = layer(
        "personal",
        LayerKind::PersonalKit,
        serde_json::json!({"theme": "dark", "nested": {"enabled": true}}),
    );
    document.source.content_digest = layer_content_digest(&document).expect("canonical digest");
    validate_layer_content_digest(&document).expect("matching digest");

    document.spec["theme"] = serde_json::json!("light");
    let error = validate_layer_content_digest(&document)
        .expect_err("a semantic layer change must invalidate its declared digest");
    assert_eq!(
        error.to_string(),
        "layer content digest does not match canonical content"
    );
}
