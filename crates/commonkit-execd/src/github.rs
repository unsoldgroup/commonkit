//! GitHub commit statuses for durable CI jobs.
//!
//! CommonKit does not use GitHub Actions. A job's outcome reaches GitHub as a
//! commit status posted from the target, with a token resolved from the secret
//! provider. Posting is best effort: GitHub being unreachable must never change
//! whether a job succeeded.

use commonkit_contracts::ExecutionManifest;
use std::collections::BTreeMap;
use std::time::Duration;

/// Statuses are namespaced so they cannot collide with another producer's
/// contexts, such as the existing `Workers Builds` check.
const CONTEXT_PREFIX: &str = "commonkit";
const ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusState {
    Pending,
    Success,
    Failure,
    Error,
}

impl StatusState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Error => "error",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Pending => "Running on a CommonKit execution target",
            Self::Success => "Passed on a CommonKit execution target",
            Self::Failure => "Failed on a CommonKit execution target",
            Self::Error => "Could not run on a CommonKit execution target",
        }
    }
}

/// Posts commit statuses for jobs whose manifest matches a declared CI task.
pub struct StatusReporter {
    client: reqwest::Client,
    api_base: String,
    token: String,
    /// Declared task identity keyed by the manifest fields that identify it.
    task_ids: BTreeMap<TaskKey, String>,
}

/// A declared task is recognised by what it runs and where, not by the revision
/// it happens to be pinned to, so a job submitted for any commit maps back to it.
type TaskKey = (String, String, Vec<String>);

fn task_key(manifest: &ExecutionManifest) -> TaskKey {
    (
        manifest.repository.clone(),
        manifest.workdir.as_str().to_owned(),
        manifest.argv.clone(),
    )
}

impl StatusReporter {
    pub fn new(
        api_base: impl Into<String>,
        token: impl Into<String>,
        tasks: &BTreeMap<String, ExecutionManifest>,
    ) -> Result<Self, StatusError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?,
            api_base: api_base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            task_ids: tasks
                .iter()
                .map(|(id, manifest)| (task_key(manifest), id.clone()))
                .collect(),
        })
    }

    /// Posts a status for this manifest, or does nothing when it is not a declared
    /// CI task or its repository is not on GitHub.
    pub async fn report(&self, manifest: &ExecutionManifest, state: StatusState) {
        let (Some(task_id), Some(slug)) = (
            self.task_ids.get(&task_key(manifest)),
            repository_slug(&manifest.repository),
        ) else {
            return;
        };
        let context = format!("{CONTEXT_PREFIX}/{task_id}");
        let url = format!(
            "{}/repos/{slug}/statuses/{}",
            self.api_base,
            manifest.repository_revision.as_str()
        );
        for attempt in 1..=ATTEMPTS {
            match self.send(&url, &context, state).await {
                Ok(()) => return,
                // A rejected request will be rejected again; only retry transport
                // failures and GitHub's own errors.
                Err(error) if !error.retryable || attempt == ATTEMPTS => {
                    eprintln!("commit status {context} not posted: {error}");
                    return;
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
                }
            }
        }
    }

    async fn send(&self, url: &str, context: &str, state: StatusState) -> Result<(), SendError> {
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "commonkit-execd")
            .json(&serde_json::json!({
                "state": state.as_str(),
                "context": context,
                "description": state.description(),
            }))
            .send()
            .await
            .map_err(|_| SendError {
                retryable: true,
                // The error is not rendered: a reqwest error can carry the URL,
                // and the token is never part of it, but the description stays
                // deliberately opaque.
                status: None,
            })?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status().as_u16();
        Err(SendError {
            retryable: status >= 500 || status == 429,
            status: Some(status),
        })
    }
}

/// The `owner/repo` slug of a GitHub HTTPS remote, if it is one.
pub fn repository_slug(repository: &str) -> Option<String> {
    let rest = repository
        .strip_prefix("https://github.com/")?
        .trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut segments = rest.split('/');
    let owner = segments.next().filter(|value| !value.is_empty())?;
    let name = segments.next().filter(|value| !value.is_empty())?;
    segments.next().is_none().then(|| format!("{owner}/{name}"))
}

#[derive(Debug)]
struct SendError {
    retryable: bool,
    status: Option<u16>,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(formatter, "GitHub responded {status}"),
            None => write!(formatter, "GitHub was unreachable"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("status reporter could not be built")]
    Client(#[from] reqwest::Error),
}
