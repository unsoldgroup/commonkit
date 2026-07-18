use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_contracts::{
    EvaluationCaseManifest, EvaluationMetric, EvaluationOutcome, EvaluationReceipt, GitRevision,
    HarnessLock, ModelLock, OptimizationLimits, PortableSourcePath, ProviderLock, ProviderSource,
    SchemaVersion, Sha256Digest, SkillDescriptor, SkillEvaluationSuite, SkillLifecycle,
    SkillOptimizationManifest, StableId,
};
use commonkit_service::{ControlToken, EventHub, ServiceStatus, router_with_skills};
use commonkit_skills::{FakeOptimizer, SkillEngine};
use tokio::sync::RwLock;
use tower::ServiceExt;

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}
fn digest(value: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", value.to_string().repeat(64))).expect("digest")
}

fn candidate_fixture(
    repository: &std::path::Path,
    state: &std::path::Path,
) -> (
    Arc<SkillEngine>,
    commonkit_contracts::SkillCandidate,
    GitRevision,
) {
    fs::create_dir_all(repository.join(".agents/skills/review")).expect("skills");
    fs::create_dir_all(repository.join("policies")).expect("policies");
    fs::write(
        repository.join(".agents/skills/review/SKILL.md"),
        "# Review\n\nFind bugs.\n",
    )
    .expect("skill");
    let policy = b"{\"adoption\":\"review_required\"}\n";
    fs::write(
        repository.join("policies/skill-optimization.policy.json"),
        policy,
    )
    .expect("policy");
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "user.name", "CommonKit Test"],
        vec!["add", ".agents", "policies"],
        vec!["commit", "-qm", "fixture"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repository)
                .args(args)
                .status()
                .expect("git")
                .success()
        );
    }
    let head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("head")
            .stdout,
    )
    .expect("utf8");
    let revision = GitRevision::parse(head.trim()).expect("revision");
    let harness = HarnessLock {
        kind: id("fake"),
        version: "1".into(),
        environment_digest: digest('1'),
    };
    let suite = SkillEvaluationSuite {
        schema_version: SchemaVersion(1),
        id: id("review-suite"),
        skill_id: id("review"),
        train: EvaluationCaseManifest {
            content_digest: digest('2'),
            case_ids: BTreeSet::from([id("train")]),
        },
        validation: EvaluationCaseManifest {
            content_digest: digest('3'),
            case_ids: BTreeSet::from([id("valid")]),
        },
        held_out: EvaluationCaseManifest {
            content_digest: digest('4'),
            case_ids: BTreeSet::from([id("held")]),
        },
        rubric_digest: digest('5'),
        harness: harness.clone(),
        metric: EvaluationMetric {
            id: id("accuracy"),
            minimum_improvement_basis_points: 100,
            maximum_held_out_regression_basis_points: 0,
            required_case_ids: BTreeSet::from([id("held")]),
        },
    };
    let source = fs::read(repository.join(".agents/skills/review/SKILL.md")).expect("source");
    let manifest = SkillOptimizationManifest {
        schema_version: SchemaVersion(1),
        id: id("optimization-one"),
        skill: SkillDescriptor {
            id: id("review"),
            source_path: PortableSourcePath::parse(".agents/skills/review/SKILL.md").expect("path"),
            source_digest: commonkit_skills::digest_bytes(&source).expect("digest"),
            package: None,
            targets: BTreeSet::from([id("codex")]),
            lifecycle: SkillLifecycle::Active,
        },
        suite_digest: suite.digest().expect("suite"),
        evidence_digests: vec![],
        provider: ProviderLock {
            id: id("fake"),
            version: "1".into(),
            adapter_contract: id("fake-v1"),
            source: ProviderSource::Container,
            package_digest: digest('8'),
            capabilities: BTreeSet::new(),
            source_revision: None,
        },
        optimizer: ModelLock {
            provider: id("fake"),
            model: "deterministic".into(),
        },
        target: ModelLock {
            provider: id("fake"),
            model: "deterministic".into(),
        },
        limits: OptimizationLimits {
            maximum_cases: 3,
            maximum_edits: 1,
            timeout_seconds: 30,
            maximum_cost_micros: 1,
        },
        policy_digest: commonkit_skills::digest_bytes(policy).expect("policy"),
        repository_revision: revision.clone(),
    };
    let evaluation = EvaluationReceipt {
        schema_version: SchemaVersion(1),
        baseline_basis_points: 5000,
        candidate_basis_points: 6000,
        held_out_baseline_basis_points: 5000,
        held_out_candidate_basis_points: 5000,
        required_cases: BTreeMap::from([(id("held"), EvaluationOutcome::Passed)]),
        cost_micros: 0,
        harness,
        scorer_digest: digest('7'),
    };
    let engine = Arc::new(SkillEngine::open(repository, state).expect("engine"));
    let candidate = engine
        .optimize(
            &manifest,
            &suite,
            &FakeOptimizer::new(b"# Review\n\nImproved.\n".to_vec(), evaluation),
        )
        .expect("candidate");
    (engine, candidate, revision)
}

async fn call(
    app: &Router,
    token: &ControlToken,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "127.0.0.1:3764")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token.expose_for_client()),
        );
    let request = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder.body(Body::from(body.to_string())).expect("request")
    } else {
        builder.body(Body::empty()).expect("request")
    };
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (status, value)
}

#[tokio::test]
async fn authenticated_service_exposes_skill_inventory() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "commonkit-service-skills-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("repository/.agents/skills/review")).expect("skills");
    fs::write(
        root.join("repository/.agents/skills/review/SKILL.md"),
        "# Review\n",
    )
    .expect("skill");
    let engine =
        Arc::new(SkillEngine::open(root.join("repository"), root.join("state")).expect("engine"));
    let token = ControlToken::generate();
    let app = router_with_skills(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        engine,
    );
    let response = app
        .clone()
        .oneshot(
            Request::get("/control/v1/skills")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body[0]["id"], "review");

    let candidates = app
        .clone()
        .oneshot(
            Request::get("/control/v1/skills/candidates")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(candidates.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(candidates.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("json"),
        serde_json::json!([])
    );

    let denied = app
        .clone()
        .oneshot(
            Request::post("/control/v1/skills/candidates/candidate-1/promotion-plans")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "confirmed": false,
                        "confirmationId": "approval-1",
                        "repositoryRevision": "a".repeat(40),
                        "approval": {
                            "approver": "reviewer-1",
                            "approvedAtUnixMs": 1,
                            "reason": "reviewed"
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(denied.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("json")["error"]["code"],
        "confirmation_required"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn authenticated_skill_lifecycle_is_redacted_consent_gated_and_reversible() {
    let root = std::env::temp_dir().join(format!(
        "commonkit-service-skill-lifecycle-{}",
        std::process::id()
    ));
    let repository = root.join("repository");
    let state = root.join("state");
    let (engine, candidate, revision) = candidate_fixture(&repository, &state);
    let token = ControlToken::generate();
    let app = router_with_skills(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        engine,
    );

    let (status, preview) = call(
        &app,
        &token,
        "POST",
        "/control/v1/skills/evidence/preview",
        Some(serde_json::json!({"content":"failure OPENAI_API_KEY=sk-secret-value"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !preview["redacted"]
            .as_str()
            .expect("redacted")
            .contains("sk-secret-value")
    );

    let import = serde_json::json!({
        "confirmed": true, "confirmationId":"evidence-import-1", "skillId":"review",
        "sourceKind":"manual_failure", "consent":"local_only",
        "retention":{"deleteAfterUnixMs":999999}, "createdAtUnixMs":1,
        "content":"review missed a correctness bug"
    });
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/control/v1/skills/evidence/import",
        Some(import),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, opportunities) = call(
        &app,
        &token,
        "POST",
        "/control/v1/skills/opportunities",
        Some(serde_json::json!({"minimumEvidence":1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opportunities[0]["skillId"], "review");
    assert_eq!(opportunities[0]["eligible"], true);

    let (status, shown) = call(
        &app,
        &token,
        "GET",
        &format!("/control/v1/skills/candidates/{}", candidate.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown["id"], candidate.id.as_str());

    let proposal = serde_json::json!({"confirmed":true,"confirmationId":"approval-1","repositoryRevision":revision,"approval":{"approver":"reviewer-1","approvedAtUnixMs":1,"reason":"reviewed"}});
    let (status, plan) = call(
        &app,
        &token,
        "POST",
        &format!(
            "/control/v1/skills/candidates/{}/promotion-plans",
            candidate.id
        ),
        Some(proposal),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, receipt) = call(
        &app,
        &token,
        "POST",
        "/control/v1/skills/promotions/apply",
        Some(serde_json::json!({"confirmed":true,"confirmationId":"apply-1","plan":plan})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md")).expect("promoted"),
        "# Review\n\nImproved.\n"
    );
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/control/v1/skills/promotions/rollback",
        Some(serde_json::json!({"confirmed":true,"confirmationId":"rollback-1","receipt":receipt})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md")).expect("rolled back"),
        "# Review\n\nFind bugs.\n"
    );

    let (status, _) = call(&app, &token, "POST", "/control/v1/skills/schedule", Some(serde_json::json!({"confirmed":true,"confirmationId":"schedule-1","enabled":true,"kind":"candidate_generation","intervalSeconds":86400,"maximumCostMicrosPerPeriod":100}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, schedule) = call(&app, &token, "GET", "/control/v1/skills/schedule", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(schedule["schedule"]["enabled"], true);
    let (status, _) = call(&app, &token, "POST", "/control/v1/skills/schedule", Some(serde_json::json!({"confirmed":true,"confirmationId":"schedule-2","enabled":false,"kind":"discovery","intervalSeconds":1,"maximumCostMicrosPerPeriod":0}))).await;
    assert_eq!(status, StatusCode::OK);
    let restarted = SkillEngine::open(&repository, &state).expect("restart");
    assert!(
        !restarted
            .schedule_status()
            .expect("schedule")
            .schedule
            .enabled
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn stale_skill_context_is_rejected_before_service_mutation() {
    let root = std::env::temp_dir().join(format!(
        "commonkit-service-skill-stale-{}",
        std::process::id()
    ));
    let repository = root.join("repository");
    let state = root.join("state");
    let (engine, candidate, revision) = candidate_fixture(&repository, &state);
    let token = ControlToken::generate();
    let app = router_with_skills(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        engine,
    );
    let proposal = serde_json::json!({"confirmed":true,"confirmationId":"approval-1","repositoryRevision":revision,"approval":{"approver":"reviewer-1","approvedAtUnixMs":1,"reason":"reviewed"}});
    let (_, plan) = call(
        &app,
        &token,
        "POST",
        &format!(
            "/control/v1/skills/candidates/{}/promotion-plans",
            candidate.id
        ),
        Some(proposal),
    )
    .await;
    fs::write(
        repository.join("policies/skill-optimization.policy.json"),
        "{\"adoption\":\"deny\"}\n",
    )
    .expect("policy change");
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/control/v1/skills/promotions/apply",
        Some(serde_json::json!({"confirmed":true,"confirmationId":"apply-1","plan":plan})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md")).expect("source"),
        "# Review\n\nFind bugs.\n"
    );
    fs::remove_dir_all(root).expect("cleanup");
}
