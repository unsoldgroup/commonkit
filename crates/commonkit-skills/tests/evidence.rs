use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::{
    ContentSensitivity, EvidenceConsent, EvidenceRetention, EvidenceSourceKind, StableId,
};
use commonkit_skills::{EvidenceImport, SkillEngine};

static NONCE: AtomicU64 = AtomicU64::new(0);

fn directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "commonkit-evidence-test-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(path.join("repository/.agents/skills/review")).expect("repository");
    fs::write(
        path.join("repository/.agents/skills/review/SKILL.md"),
        "# Review\n",
    )
    .expect("skill");
    path
}

#[test]
fn evidence_is_redacted_previewed_and_persisted_as_local_sensitive() {
    let root = directory();
    let engine = SkillEngine::open(root.join("repository"), root.join("state")).expect("engine");
    let input =
        b"failure: token=super-secret-value\npath=/Users/developer/private/project\nkeep=this detail\n";

    let preview = engine.preview_evidence(input).expect("preview");
    assert!(!preview.redacted.contains("super-secret-value"));
    assert!(!preview.redacted.contains("/Users/developer/private/project"));
    assert!(preview.redacted.contains("keep=this detail"));
    assert_eq!(preview.findings.len(), 2);

    let envelope = engine
        .import_evidence(EvidenceImport {
            skill_id: StableId::parse("review").expect("id"),
            source_kind: EvidenceSourceKind::ManualFailure,
            consent: EvidenceConsent::LocalOnly,
            retention: EvidenceRetention {
                delete_after_unix_ms: 2_000,
            },
            created_at_unix_ms: 1_000,
            bytes: input.to_vec(),
        })
        .expect("import");
    assert_eq!(envelope.sensitivity, ContentSensitivity::LocalSensitive);
    assert_eq!(engine.evidence(&envelope.id).expect("reload"), envelope);
    assert_eq!(
        engine.evidence_preview(&envelope.id).expect("content"),
        preview.redacted
    );
    let opportunities = engine.opportunities(1).expect("opportunities");
    assert_eq!(opportunities.len(), 1);
    assert_eq!(opportunities[0].skill_id.as_str(), "review");
    assert_eq!(opportunities[0].reviewed_evidence_count, 1);
    assert!(opportunities[0].eligible);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn evidence_requires_future_retention_and_explicit_provider_consent() {
    let root = directory();
    let engine = SkillEngine::open(root.join("repository"), root.join("state")).expect("engine");
    let error = engine
        .import_evidence(EvidenceImport {
            skill_id: StableId::parse("review").expect("id"),
            source_kind: EvidenceSourceKind::CodexSession,
            consent: EvidenceConsent::LocalOnly,
            retention: EvidenceRetention {
                delete_after_unix_ms: 999,
            },
            created_at_unix_ms: 1_000,
            bytes: b"safe".to_vec(),
        })
        .expect_err("expired retention");
    assert_eq!(
        error.to_string(),
        "evidence retention must end after creation"
    );
    fs::remove_dir_all(root).expect("cleanup");
}
