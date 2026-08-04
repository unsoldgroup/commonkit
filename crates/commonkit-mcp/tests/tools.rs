use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use commonkit_contracts::*;
use commonkit_mcp::{
    AboutMeResolveInput, AboutMeSearchInput, AboutMeSuggestionInput,
    ApplyPlanInput, BackendFuture, CommonKitMcp, ConsentInput, ControlBackend, ExecutionBackend,
    ExecutionContext, ProposeSkillCanaryApplyInput, ProposeSkillCanaryRollbackInput,
    ProposeSkillPromotionInput, ReadInput, RelayReconcileInput, RelayReviewInput,
    ShowSkillCandidateInput, SkillEvidencePreviewInput, SkillOpportunitiesInput, SubmitTaskInput,
    TargetConsentInput,
};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

#[derive(Default)]
struct FakeBackend {
    applies: Mutex<Vec<(String, String)>>,
    posts: Mutex<Vec<String>>,
}

impl ControlBackend for FakeBackend {
    fn get<'a>(&'a self, path: &'a str) -> BackendFuture<'a> {
        Box::pin(async move { Ok(json!({"path": path, "state": "healthy"})) })
    }

    fn apply(&self, input: ApplyPlanInput) -> BackendFuture<'_> {
        self.applies
            .lock()
            .expect("applies")
            .push((input.target_id.clone(), input.plan_id.clone()));
        Box::pin(async move { Ok(json!({"planId": input.plan_id, "status": "running"})) })
    }

    fn post<'a>(&'a self, path: &'a str, _input: serde_json::Value) -> BackendFuture<'a> {
        self.posts.lock().expect("posts").push(path.into());
        Box::pin(async move { Ok(json!({"path": path})) })
    }
}
#[derive(Default)]
struct FakeExecution {
    paths: Mutex<Vec<String>>,
}
impl ExecutionBackend for FakeExecution {
    fn request(
        &self,
        _: &'static str,
        path: String,
        _: Option<serde_json::Value>,
    ) -> BackendFuture<'_> {
        self.paths.lock().unwrap().push(path);
        Box::pin(async { Ok(json!({"jobId":"job_1"})) })
    }
}
fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}
fn manifest() -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: "https://example.invalid/repo.git".into(),
        repository_revision: GitRevision::parse("a".repeat(40)).unwrap(),
        workspace_bundle_digest: None,
        argv: vec!["cargo".into(), "test".into()],
        workdir: PortableSourcePath::parse("repo").unwrap(),
        secret_refs: vec![],
        timeout_seconds: 60,
        cancel_grace_seconds: 1,
        resources: ResourceRequirements {
            cpu_millis: 100,
            memory_mib: 64,
            disk_mib: 10,
        },
        required_capabilities: BTreeSet::new(),
        loadout_digest: digest('b'),
        execution_profile_digest: digest('c'),
        retry: RetryPolicy {
            max_attempts: 1,
            retryable_exit_codes: BTreeSet::new(),
        },
        checkpoint_enabled: false,
        artifacts: ArtifactPolicy {
            globs: vec![],
            retention_seconds: 60,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Deny,
        repository_write: false,
        browser: None,
    }
}

#[test]
fn publishes_stable_initial_tool_names() {
    let names = CommonKitMcp::published_tool_names();
    assert_eq!(
        names,
        [
            "commonkit_about_me_get_summary",
            "commonkit_about_me_resolve_conflict",
            "commonkit_about_me_search",
            "commonkit_about_me_suggest",
            "commonkit_apply_plan",
            "commonkit_cancel_job",
            "commonkit_compose",
            "commonkit_credentials_readiness",
            "commonkit_explain",
            "commonkit_export_diagnostics",
            "commonkit_get_artifact_access",
            "commonkit_get_execution_context",
            "commonkit_get_execution_targets",
            "commonkit_get_job",
            "commonkit_get_job_events",
            "commonkit_get_principal",
            "commonkit_get_status",
            "commonkit_list_job_artifacts",
            "commonkit_list_skill_candidates",
            "commonkit_list_skills",
            "commonkit_plan_sync",
            "commonkit_preview_skill_evidence",
            "commonkit_propose_skill_canary_apply",
            "commonkit_propose_skill_canary_rollback",
            "commonkit_propose_skill_promotion",
            "commonkit_relay_reconcile",
            "commonkit_relay_status",
            "commonkit_resume_job",
            "commonkit_retry_job",
            "commonkit_rollback",
            "commonkit_schedule_status",
            "commonkit_schedule_update",
            "commonkit_show_skill_candidate",
            "commonkit_skill_opportunities",
            "commonkit_snapshot_create",
            "commonkit_snapshot_restore",
            "commonkit_submit_task",
            "commonkit_verify",
        ]
    );
    assert!(!names.iter().any(|name| matches!(
        name.as_str(),
        "commonkit_optimize_skill"
            | "commonkit_skill_provider_upgrade"
            | "commonkit_apply_skill_promotion"
            | "commonkit_rollback_skill_promotion"
    )));
}

#[tokio::test]
async fn about_me_tools_use_the_profile_service_boundary() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());

    server.about_me_summary().await.unwrap();
    server
        .about_me_search(Parameters(AboutMeSearchInput {
            query: "answer style".into(),
            categories: vec!["communication".into()],
            limit: 5,
        }))
        .await
        .unwrap();
    server
        .about_me_suggest(Parameters(AboutMeSuggestionInput {
            topic_key: "workflow/testing".into(),
            category: "workflow".into(),
            text: "I prefer tests first.".into(),
            evidence_quote: "Please use tests first.".into(),
        }))
        .await
        .unwrap();
    server
        .about_me_resolve_conflict(Parameters(AboutMeResolveInput {
            active_claim_id: "claim-1".into(),
            expected_revision: 2,
            replacement_text: "I prefer concise answers.".into(),
            evidence_quote: "Keep it concise.".into(),
            confirmed: true,
        }))
        .await
        .unwrap();

    assert_eq!(
        backend.posts.lock().unwrap().as_slice(),
        [
            "/control/v1/about-me/search",
            "/control/v1/about-me/suggestions",
            "/control/v1/about-me/conflicts/resolve"
        ]
    );
}

#[tokio::test]
async fn skill_canary_tools_are_proposal_only_and_never_contact_mutation_backend() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    let apply = server
        .propose_skill_canary_apply(Parameters(ProposeSkillCanaryApplyInput {
            run_id: "run-1".into(),
            deployment: json!({"candidateId":"candidate-1"}),
        }))
        .await
        .expect("proposal");
    assert_eq!(
        apply.structured_content.expect("structured")["proposalOnly"],
        true
    );
    let rollback = server
        .propose_skill_canary_rollback(Parameters(ProposeSkillCanaryRollbackInput {
            run_id: "run-1".into(),
            deployment_receipt_id: "sha256:receipt".into(),
        }))
        .await
        .expect("proposal");
    assert_eq!(
        rollback.structured_content.expect("structured")["proposalOnly"],
        true
    );
    assert!(backend.posts.lock().expect("posts").is_empty());
    assert!(backend.applies.lock().expect("applies").is_empty());
}

#[tokio::test]
async fn read_tools_return_structured_daemon_data() {
    let server = CommonKitMcp::new(Arc::new(FakeBackend::default()));
    let result = server.get_status().await.expect("status");
    assert_eq!(
        result.structured_content.expect("structured")["state"],
        "healthy"
    );
    let principal = server.get_principal().await.expect("principal");
    assert_eq!(
        principal.structured_content.expect("structured")["path"],
        "/control/v1/principal"
    );
    let inventory = server.list_skills().await.expect("skill inventory");
    assert_eq!(
        inventory.structured_content.expect("structured")["path"],
        "/control/v1/skills"
    );
    let candidates = server
        .list_skill_candidates()
        .await
        .expect("skill candidates");
    assert_eq!(
        candidates.structured_content.expect("structured")["path"],
        "/control/v1/skills/candidates"
    );
}

#[tokio::test]
async fn skill_tools_are_read_or_proposal_only() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    server
        .show_skill_candidate(Parameters(ShowSkillCandidateInput {
            candidate_id: "candidate-1".into(),
        }))
        .await
        .expect("show");
    server
        .preview_skill_evidence(Parameters(SkillEvidencePreviewInput {
            content: "redacted sample".into(),
        }))
        .await
        .expect("preview");
    server
        .skill_opportunities(Parameters(SkillOpportunitiesInput {
            minimum_evidence: 3,
        }))
        .await
        .expect("opportunities");
    let denied = server
        .propose_skill_promotion(Parameters(ProposeSkillPromotionInput {
            candidate_id: "candidate-1".into(),
            repository_revision: "a".repeat(40),
            approver: "reviewer-1".into(),
            approved_at_unix_ms: 1,
            reason: "reviewed".into(),
            confirmed: false,
            confirmation_id: "approval-1".into(),
        }))
        .await
        .expect("denied");
    assert_eq!(denied.is_error, Some(true));
    assert!(backend.applies.lock().expect("applies").is_empty());
}

#[tokio::test]
async fn apply_refuses_missing_confirmation_before_calling_the_backend() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    let denied = server
        .apply_plan(Parameters(ApplyPlanInput {
            target_id: "workstation-a".into(),
            plan_id: format!("sha256:{}", "a".repeat(64)),
            confirmed: false,
            confirmation_id: "user-approved".into(),
            idempotency_key: "request-1".into(),
        }))
        .await
        .expect("denied");
    assert_eq!(denied.is_error, Some(true));
    assert!(backend.applies.lock().expect("applies").is_empty());
    assert!(backend.posts.lock().expect("posts").is_empty());
}

#[tokio::test]
async fn plan_verify_and_apply_are_bound_to_one_explicit_target() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());

    server
        .plan_sync(Parameters(TargetConsentInput {
            target_id: "workstation-a".into(),
            confirmed: true,
            confirmation_id: "plan-a".into(),
            idempotency_key: "plan-request-a".into(),
        }))
        .await
        .expect("plan");
    server
        .verify(Parameters(ReadInput {
            target_id: Some("workstation-a".into()),
            pointer: None,
        }))
        .await
        .expect("verify");
    server
        .apply_plan(Parameters(ApplyPlanInput {
            target_id: "workstation-a".into(),
            plan_id: format!("sha256:{}", "a".repeat(64)),
            confirmed: true,
            confirmation_id: "apply-a".into(),
            idempotency_key: "apply-request-a".into(),
        }))
        .await
        .expect("apply");

    assert_eq!(
        *backend.posts.lock().expect("posts"),
        [
            "/control/v1/targets/workstation-a/sync/plan",
            "/control/v1/targets/workstation-a/verify",
        ]
    );
    assert_eq!(
        *backend.applies.lock().expect("applies"),
        [("workstation-a".into(), format!("sha256:{}", "a".repeat(64)))]
    );
}

#[tokio::test]
async fn verify_rejects_an_omitted_target_before_contacting_the_backend() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    let result = server
        .verify(Parameters(ReadInput {
            target_id: None,
            pointer: None,
        }))
        .await
        .expect("structured rejection");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.expect("content")["code"],
        "target_required"
    );
    assert!(backend.posts.lock().expect("posts").is_empty());
}

#[tokio::test]
async fn every_mutation_tool_requires_consent_before_backend_execution() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    for result in [
        server
            .plan_sync(Parameters(TargetConsentInput::denied("local")))
            .await,
        server
            .snapshot_create(Parameters(ConsentInput::denied()))
            .await,
        server
            .snapshot_restore(Parameters(ConsentInput::denied()))
            .await,
        server.rollback(Parameters(ConsentInput::denied())).await,
    ] {
        let result = result.expect("structured denial");
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.expect("structured")["code"],
            "confirmation_required"
        );
    }
    let denied = server
        .relay_reconcile(Parameters(RelayReconcileInput {
            target_id: "local".into(),
            confirmed: false,
            confirmation_id: String::new(),
            idempotency_key: String::new(),
            review: None,
        }))
        .await
        .expect("relay denial");
    assert_eq!(
        denied.structured_content.expect("structured")["code"],
        "invalid_input"
    );
    assert!(backend.applies.lock().expect("applies").is_empty());
    assert!(backend.posts.lock().expect("posts").is_empty());
}

#[tokio::test]
async fn relay_proposal_and_consent_forward_only_target_owned_review_authority() {
    let backend = Arc::new(FakeBackend::default());
    let server = CommonKitMcp::new(backend.clone());
    server
        .relay_reconcile(Parameters(RelayReconcileInput {
            target_id: "local".into(),
            confirmed: false,
            confirmation_id: "relay-review-one".into(),
            idempotency_key: "relay-request-one".into(),
            review: None,
        }))
        .await
        .expect("proposal");
    let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    server
        .relay_reconcile(Parameters(RelayReconcileInput {
            target_id: "local".into(),
            confirmed: true,
            confirmation_id: "relay-review-one".into(),
            idempotency_key: "relay-request-one".into(),
            review: Some(RelayReviewInput {
                declaration_digest: digest.into(),
                provider_inputs_digest: digest.into(),
                ownership_map_digest: digest.into(),
                artifact_set_digest: digest.into(),
                plan_digest: digest.into(),
            }),
        }))
        .await
        .expect("consent");
    assert_eq!(backend.posts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn read_tools_forward_only_bounded_structured_inputs() {
    let server = CommonKitMcp::new(Arc::new(FakeBackend::default()));
    let result = server
        .compose(Parameters(ReadInput {
            target_id: Some("local".into()),
            pointer: None,
        }))
        .await
        .expect("compose");
    assert_eq!(
        result.structured_content.expect("structured")["path"],
        "/control/v1/compose"
    );
}

#[tokio::test]
async fn remote_mcp_submits_only_repository_declared_tasks() {
    let execution = Arc::new(FakeExecution::default());
    let server = CommonKitMcp::new(Arc::new(FakeBackend::default())).with_execution(
        execution.clone(),
        ExecutionContext {
            issue_id: "USG-46".into(),
            plan: "Run contracts".into(),
            skills: vec!["durable-objects".into()],
            tasks: BTreeMap::from([("verify".into(), manifest())]),
        },
    );
    let context = server.get_execution_context().await.unwrap();
    assert_eq!(context.structured_content.unwrap()["issueId"], "USG-46");
    let denied = server
        .submit_task(Parameters(SubmitTaskInput {
            task_id: "arbitrary-shell".into(),
            idempotency_key: "1".into(),
        }))
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    assert!(execution.paths.lock().unwrap().is_empty());
    let accepted = server
        .submit_task(Parameters(SubmitTaskInput {
            task_id: "verify".into(),
            idempotency_key: "2".into(),
        }))
        .await
        .unwrap();
    assert_eq!(accepted.structured_content.unwrap()["jobId"], "job_1");
}
