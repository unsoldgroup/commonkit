//! GitHub push and pull-request events as CI job submissions.
//!
//! CommonKit does not use GitHub Actions. GitHub's only role here is to say that
//! a revision exists; what runs against it is the repository's own committed task
//! declaration, and it runs on a capability-labelled Execution Target. The event
//! body is untrusted input: nothing in it selects a command, a target, or a
//! resource ceiling. It contributes exactly one value, the revision.

use crate::{ApiError, ApiResult, ExecutionPolicy, now_ms};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use commonkit_contracts::{ExecutionManifest, GitRevision};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The declared tasks a delivery may submit, and the shared secret that proves the
/// delivery came from GitHub.
pub struct WebhookConfig {
    secret: String,
    tasks: BTreeMap<String, ExecutionManifest>,
}

impl WebhookConfig {
    pub fn new(secret: impl Into<String>, tasks: BTreeMap<String, ExecutionManifest>) -> Self {
        Self {
            secret: secret.into(),
            tasks,
        }
    }
}

pub(crate) async fn receive(
    State(state): State<crate::ApiState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let config = state.webhook.as_ref().ok_or(ApiError {
        status: StatusCode::NOT_FOUND,
        code: "webhook_not_configured",
    })?;
    if state.require_tls
        && headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            != Some("https")
    {
        return Err(ApiError::forbidden("tls_required"));
    }
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !signature_matches(&config.secret, &body, signature) {
        let _ = state.scheduler.lock().unwrap().audit(
            now_ms(),
            "github-webhook",
            "submit",
            "execution-api",
            false,
            "webhook_signature_invalid",
        );
        return Err(ApiError::unauthorized("webhook_signature_invalid"));
    }
    let event = headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    // A delivery that carries no revision to run is still a delivery GitHub must
    // see accepted, or it marks the hook unhealthy and stops sending.
    let Some((slug, revision)) = trigger(event, &body)? else {
        return Ok((StatusCode::OK, axum::Json(json!({"submitted": []}))));
    };

    let mut submitted = Vec::new();
    for (task_id, declared) in &config.tasks {
        if crate::github::repository_slug(&declared.repository).as_deref() != Some(slug.as_str()) {
            continue;
        }
        let mut manifest = declared.clone();
        manifest.repository_revision = revision.clone();
        manifest
            .validate()
            .map_err(|_| ApiError::forbidden("declared_task_invalid"))?;
        if let Some(policy) = &state.policy {
            ExecutionPolicy::validate(policy, &manifest)?;
        }
        // One job per task per revision, however many times GitHub redelivers.
        let key = format!("{task_id}:{}", revision.as_str());
        let outcome = state
            .scheduler
            .lock()
            .unwrap()
            .submit(&manifest, &key, now_ms())?;
        let (job_id, created) = match outcome {
            commonkit_execution::SubmitOutcome::Created(id) => (id, true),
            commonkit_execution::SubmitOutcome::Existing(id) => (id, false),
        };
        submitted.push(json!({"taskId": task_id, "jobId": job_id, "created": created}));
    }
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(json!({"revision": revision.as_str(), "submitted": submitted})),
    ))
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
}
#[derive(Deserialize)]
struct PushEvent {
    repository: Repository,
    after: String,
    deleted: Option<bool>,
}
#[derive(Deserialize)]
struct PullRequestEvent {
    repository: Repository,
    action: String,
    pull_request: PullRequest,
}
#[derive(Deserialize)]
struct PullRequest {
    head: Head,
}
#[derive(Deserialize)]
struct Head {
    sha: String,
}

/// The `(owner/repo, revision)` a delivery asks CommonKit to run, if it asks for
/// anything at all.
fn trigger(event: &str, body: &[u8]) -> Result<Option<(String, GitRevision)>, ApiError> {
    let malformed = || ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "webhook_payload_invalid",
    };
    match event {
        "push" => {
            let event: PushEvent = serde_json::from_slice(body).map_err(|_| malformed())?;
            // A branch deletion reports a revision of all zeroes, which is not a
            // commit anyone can check out.
            if event.deleted.unwrap_or(false) {
                return Ok(None);
            }
            match GitRevision::parse(event.after) {
                Ok(revision) if !revision.as_str().chars().all(|c| c == '0') => {
                    Ok(Some((event.repository.full_name, revision)))
                }
                _ => Ok(None),
            }
        }
        "pull_request" => {
            let event: PullRequestEvent = serde_json::from_slice(body).map_err(|_| malformed())?;
            if !matches!(
                event.action.as_str(),
                "opened" | "synchronize" | "reopened" | "ready_for_review"
            ) {
                return Ok(None);
            }
            let revision =
                GitRevision::parse(event.pull_request.head.sha).map_err(|_| malformed())?;
            Ok(Some((event.repository.full_name, revision)))
        }
        _ => Ok(None),
    }
}

fn signature_matches(secret: &str, body: &[u8], signature: &str) -> bool {
    let Some(provided) = signature.strip_prefix("sha256=") else {
        return false;
    };
    let expected = hmac_sha256(secret.as_bytes(), body);
    use subtle::ConstantTimeEq;
    provided.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// HMAC-SHA256 as GitHub computes it, over a secret of any length.
fn hmac_sha256(key: &[u8], message: &[u8]) -> String {
    let mut block = [0_u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in block.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}
