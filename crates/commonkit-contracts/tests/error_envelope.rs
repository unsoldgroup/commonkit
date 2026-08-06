use std::collections::BTreeMap;

use commonkit_contracts::{
    CommonKitErrorBody, ErrorCategory, ErrorCode, ErrorEnvelope, SchemaVersion,
};

#[test]
fn serializes_a_stable_redacted_error_envelope() {
    let envelope = ErrorEnvelope {
        schema_version: SchemaVersion(1),
        error: CommonKitErrorBody {
            code: ErrorCode::ConfigInvalidDocument,
            message: "Layer document is invalid".into(),
            category: ErrorCategory::Configuration,
            retryable: false,
            details: BTreeMap::from([("layerId".into(), serde_json::json!("personal"))]),
            causes: vec![],
            remediation: vec![],
            correlation_id: None,
        },
    };

    assert_eq!(
        serde_json::to_value(envelope).expect("serialize"),
        serde_json::json!({
            "schemaVersion": 1,
            "error": {
                "code": "CK_CONFIG_001_INVALID_DOCUMENT",
                "message": "Layer document is invalid",
                "category": "configuration",
                "retryable": false,
                "details": {"layerId": "personal"},
                "causes": [],
                "remediation": []
            }
        })
    );
}
