mod support;

use commonkit_config::{layer_content_digest, validate_layer_content_digest};
use commonkit_contracts::LayerKind;
use support::layer;

#[test]
fn layer_digest_binds_canonical_content_without_hashing_itself() {
    let mut document = layer(
        "personal",
        LayerKind::PersonalKit,
        serde_json::json!({"contextBudget": {"maxTotalTokens": 1000}}),
    );
    document.source.content_digest = layer_content_digest(&document).expect("canonical digest");
    validate_layer_content_digest(&document).expect("matching digest");

    document.spec["contextBudget"]["maxTotalTokens"] = serde_json::json!(500);
    let error = validate_layer_content_digest(&document)
        .expect_err("a semantic layer change must invalidate its declared digest");
    assert_eq!(
        error.to_string(),
        "layer content digest does not match canonical content"
    );
}
