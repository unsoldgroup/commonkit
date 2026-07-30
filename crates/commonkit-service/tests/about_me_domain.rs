use std::sync::Arc;

use commonkit_reconcile::PlanStore;
use commonkit_service::ProductionDomainRegistry;
use serde_json::json;

#[test]
fn production_profile_is_encrypted_scoped_and_reviewed_before_agent_recall() {
    let root = tempfile::tempdir().unwrap();
    let key = root.path().join("profile.key");
    std::fs::write(&key, b"test-only-profile-key").unwrap();
    let config = root.path().join("headless.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "aboutMe": {
                "database": root.path().join("about-me.sqlite"),
                "keyReference": format!("file://{}", key.display()),
                "bwsExecutable": null,
                "loadoutId": "personal",
                "projectId": "commonkit",
                "agentId": "codex"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let plans = Arc::new(PlanStore::open(root.path().join("plans")).unwrap());
    let registry =
        ProductionDomainRegistry::load_optional(&config, plans, root.path().join("receipts"))
            .unwrap()
            .into_headless();
    let profile = registry.about_me.unwrap();

    let draft = profile
        .create_draft(json!({
            "expectedRevision": 0,
            "summary": "Call me Al.",
            "claims": [{
                "topicKey": "communication/style",
                "category": "communication",
                "text": "I prefer plain language."
            }]
        }))
        .unwrap();
    profile
        .publish_draft(json!({
            "draftId": draft["id"],
            "expectedRevision": 0,
            "confirmed": true
        }))
        .unwrap();

    let found = profile
        .search(json!({"query":"plain language","categories":[],"limit":5}))
        .unwrap();
    assert_eq!(found["claims"].as_array().unwrap().len(), 1);
    assert_eq!(profile.inspect().unwrap()["pendingSuggestions"], 0);

    let bytes = std::fs::read(root.path().join("about-me.sqlite")).unwrap();
    assert!(
        !bytes
            .windows(b"plain language".len())
            .any(|window| window == b"plain language")
    );
}
