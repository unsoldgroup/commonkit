use std::collections::BTreeMap;

use commonkit_contracts::{SchemaVersion, Sha256Digest, StableId};
use commonkit_platform::{
    EvidenceOutcome, NativeValidationEvidence, NativeValidationStep, ValidationPlatform,
    portable_context_support_ready,
};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn evidence(platform: ValidationPlatform) -> NativeValidationEvidence {
    NativeValidationEvidence {
        schema_version: SchemaVersion(1),
        platform,
        architecture: "native".into(),
        repository_revision: digest('a'),
        environment_digest: digest('b'),
        provider_digests: BTreeMap::from([(StableId::parse("docling").unwrap(), digest('c'))]),
        steps: vec![NativeValidationStep {
            id: StableId::parse("installed-lifecycle").unwrap(),
            outcome: EvidenceOutcome::Passed,
            artifact_hash: digest('d'),
        }],
        recorded_at_unix_ms: 100,
    }
}

#[test]
fn support_requires_passing_native_evidence_for_all_three_platforms() {
    let revision = digest('a');
    let mut records = vec![
        evidence(ValidationPlatform::MacOs),
        evidence(ValidationPlatform::Linux),
    ];
    assert!(!portable_context_support_ready(&records, &revision));
    records.push(evidence(ValidationPlatform::Windows));
    assert!(portable_context_support_ready(&records, &revision));
    records[2].steps[0].outcome = EvidenceOutcome::Skipped;
    assert!(!portable_context_support_ready(&records, &revision));
}

#[test]
fn evidence_validation_rejects_secret_bearing_metadata() {
    let mut record = evidence(ValidationPlatform::MacOs);
    record.architecture = "API_TOKEN=super-secret-token-value".into();
    assert!(record.validate(&digest('a')).is_err());
}
