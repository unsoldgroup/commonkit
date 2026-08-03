//! The commit-status poster, against a stub GitHub.
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use commonkit_contracts::*;
use commonkit_execd::github::{StatusReporter, StatusState, repository_slug};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorded {
    calls: Vec<(String, Value, HeaderMap)>,
    /// Status codes to return, consumed in order; success once exhausted.
    responses: Vec<StatusCode>,
}

type Stub = Arc<Mutex<Recorded>>;

async fn statuses(
    State(stub): State<Stub>,
    Path((owner, name, sha)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: String,
) -> StatusCode {
    let mut stub = stub.lock().unwrap();
    let payload = serde_json::from_str(&body).unwrap();
    stub.calls
        .push((format!("{owner}/{name}@{sha}"), payload, headers));
    if stub.responses.is_empty() {
        StatusCode::CREATED
    } else {
        stub.responses.remove(0)
    }
}

/// Serves the stub on a loopback port and returns its base URL.
async fn serve(stub: Stub) -> String {
    let router = Router::new()
        .route("/repos/{owner}/{name}/statuses/{sha}", post(statuses))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{address}")
}

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn manifest(repository: &str) -> ExecutionManifest {
    ExecutionManifest {
        schema_version: SchemaVersion(1),
        repository: repository.to_owned(),
        repository_revision: GitRevision::parse("b".repeat(40)).unwrap(),
        workspace_bundle_digest: None,
        argv: vec!["cargo".into(), "test".into()],
        workdir: PortableSourcePath::parse(".").unwrap(),
        secret_refs: Vec::new(),
        timeout_seconds: 30,
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
            globs: vec!["diagnostics/**".into()],
            retention_seconds: 60,
            max_bytes: 1024,
        },
        network_policy: NetworkPolicy::Deny,
        repository_write: false,
        browser: None,
    }
}

fn declared(manifest: &ExecutionManifest) -> BTreeMap<String, ExecutionManifest> {
    BTreeMap::from([("ci-cargo-test".to_owned(), manifest.clone())])
}

const REPOSITORY: &str = "https://github.com/unsoldgroup/commonkit.git";

#[tokio::test]
async fn a_declared_task_posts_a_namespaced_status_for_its_revision() {
    let stub = Stub::default();
    let base = serve(stub.clone()).await;
    let manifest = manifest(REPOSITORY);
    let reporter = StatusReporter::new(base, "secret-token", &declared(&manifest)).unwrap();

    reporter.report(&manifest, StatusState::Success).await;

    let recorded = stub.lock().unwrap();
    assert_eq!(recorded.calls.len(), 1);
    let (path, payload, headers) = &recorded.calls[0];
    assert_eq!(path, &format!("unsoldgroup/commonkit@{}", "b".repeat(40)));
    assert_eq!(payload["state"], "success");
    // Namespaced so it cannot collide with the existing `Workers Builds` status.
    assert_eq!(payload["context"], "commonkit/ci-cargo-test");
    assert_eq!(headers.get("authorization").unwrap(), "Bearer secret-token");
}

#[tokio::test]
async fn every_job_state_maps_to_a_github_state() {
    let stub = Stub::default();
    let base = serve(stub.clone()).await;
    let manifest = manifest(REPOSITORY);
    let reporter = StatusReporter::new(base, "secret-token", &declared(&manifest)).unwrap();

    for state in [
        StatusState::Pending,
        StatusState::Success,
        StatusState::Failure,
        StatusState::Error,
    ] {
        reporter.report(&manifest, state).await;
    }

    let recorded = stub.lock().unwrap();
    let states: Vec<_> = recorded
        .calls
        .iter()
        .map(|(_, payload, _)| payload["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(states, ["pending", "success", "failure", "error"]);
}

#[tokio::test]
async fn a_server_error_is_retried_until_it_succeeds() {
    let stub = Stub::default();
    stub.lock().unwrap().responses = vec![StatusCode::INTERNAL_SERVER_ERROR];
    let base = serve(stub.clone()).await;
    let manifest = manifest(REPOSITORY);
    let reporter = StatusReporter::new(base, "secret-token", &declared(&manifest)).unwrap();

    reporter.report(&manifest, StatusState::Success).await;

    assert_eq!(stub.lock().unwrap().calls.len(), 2);
}

/// A rejected request will be rejected again, so retrying only wastes the window.
#[tokio::test]
async fn a_rejected_request_is_not_retried() {
    let stub = Stub::default();
    stub.lock().unwrap().responses = vec![StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED];
    let base = serve(stub.clone()).await;
    let manifest = manifest(REPOSITORY);
    let reporter = StatusReporter::new(base, "secret-token", &declared(&manifest)).unwrap();

    reporter.report(&manifest, StatusState::Success).await;

    assert_eq!(stub.lock().unwrap().calls.len(), 1);
}

#[tokio::test]
async fn a_manifest_that_is_not_a_declared_task_is_not_reported() {
    let stub = Stub::default();
    let base = serve(stub.clone()).await;
    let declared_manifest = manifest(REPOSITORY);
    let reporter =
        StatusReporter::new(base, "secret-token", &declared(&declared_manifest)).unwrap();
    let mut undeclared = declared_manifest.clone();
    undeclared.argv = vec!["curl".into(), "https://example.invalid".into()];

    reporter.report(&undeclared, StatusState::Success).await;

    assert!(stub.lock().unwrap().calls.is_empty());
}

/// A task is recognised by what it runs, not the commit it happens to be pinned to.
#[tokio::test]
async fn a_declared_task_is_reported_at_any_revision() {
    let stub = Stub::default();
    let base = serve(stub.clone()).await;
    let declared_manifest = manifest(REPOSITORY);
    let reporter =
        StatusReporter::new(base, "secret-token", &declared(&declared_manifest)).unwrap();
    let mut other_revision = declared_manifest.clone();
    other_revision.repository_revision = GitRevision::parse("c".repeat(40)).unwrap();

    reporter.report(&other_revision, StatusState::Success).await;

    let recorded = stub.lock().unwrap();
    assert_eq!(recorded.calls.len(), 1);
    assert!(recorded.calls[0].0.ends_with(&"c".repeat(40)));
}

#[tokio::test]
async fn a_repository_that_is_not_on_github_is_not_reported() {
    let stub = Stub::default();
    let base = serve(stub.clone()).await;
    let manifest = manifest("/srv/mirrors/local.git");
    let reporter = StatusReporter::new(base, "secret-token", &declared(&manifest)).unwrap();

    reporter.report(&manifest, StatusState::Success).await;

    assert!(stub.lock().unwrap().calls.is_empty());
}

#[test]
fn repository_slugs_are_parsed_only_from_github_https_remotes() {
    assert_eq!(
        repository_slug("https://github.com/unsoldgroup/commonkit.git").as_deref(),
        Some("unsoldgroup/commonkit")
    );
    assert_eq!(
        repository_slug("https://github.com/unsoldgroup/commonkit").as_deref(),
        Some("unsoldgroup/commonkit")
    );
    for value in [
        "git@github.com:unsoldgroup/commonkit.git",
        "https://gitlab.com/unsoldgroup/commonkit.git",
        "https://github.com/unsoldgroup",
        "https://github.com/unsoldgroup/commonkit/extra",
        "/srv/mirrors/local.git",
    ] {
        assert!(repository_slug(value).is_none(), "{value} was parsed");
    }
}
