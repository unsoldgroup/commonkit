use commonkit_contracts::portable_context::RepositoryRole;
use commonkit_contracts::{ContractError, Sha256Digest, StableId, digest_domain_json};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubRepositoryRequest {
    pub id: StableId,
    pub role: RepositoryRole,
    pub owner: String,
    /// Immutable GitHub node ID for the expected repository owner.
    pub owner_node_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubRepositoryObservation {
    pub account_node_id: String,
    pub repository_node_id: String,
    pub owner: String,
    pub name: String,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubRepositoryPlan {
    pub request: GitHubRepositoryRequest,
    pub observed: Option<GitHubRepositoryObservation>,
    pub confirmation_digest: Sha256Digest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSemantic<'a> {
    request: &'a GitHubRepositoryRequest,
    observed: &'a Option<GitHubRepositoryObservation>,
}

impl GitHubRepositoryPlan {
    pub fn digest(&self) -> Result<Sha256Digest, ContractError> {
        digest_domain_json(
            "commonkit.github-onboarding-plan.v1",
            &PlanSemantic {
                request: &self.request,
                observed: &self.observed,
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryApplyDisposition {
    Created,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryApplyResult {
    pub disposition: RepositoryApplyDisposition,
    pub observation: GitHubRepositoryObservation,
}

pub trait GitHubTransport {
    fn inspect_repository(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<Option<GitHubRepositoryObservation>, GitHubOnboardingError>;

    fn create_private_repository(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<GitHubRepositoryObservation, GitHubOnboardingError>;
}

pub struct GitHubOnboardingAdapter<'a, T> {
    transport: &'a T,
}

impl<'a, T: GitHubTransport> GitHubOnboardingAdapter<'a, T> {
    pub fn new(transport: &'a T) -> Self {
        Self { transport }
    }

    pub fn inspect_and_plan(
        &self,
        request: GitHubRepositoryRequest,
    ) -> Result<GitHubRepositoryPlan, GitHubOnboardingError> {
        validate_request(&request)?;
        let observed = self
            .transport
            .inspect_repository(&request.owner, &request.name)?;
        if let Some(observation) = &observed {
            validate_observation(&request, observation)?;
        }
        let mut plan = GitHubRepositoryPlan {
            request,
            observed,
            confirmation_digest: placeholder_digest(),
        };
        plan.confirmation_digest = plan.digest()?;
        Ok(plan)
    }

    pub fn apply(
        &self,
        plan: &GitHubRepositoryPlan,
        confirmation_digest: &Sha256Digest,
    ) -> Result<RepositoryApplyResult, GitHubOnboardingError> {
        let current_digest = plan.digest()?;
        if confirmation_digest != &plan.confirmation_digest
            || confirmation_digest != &current_digest
        {
            return Err(GitHubOnboardingError::ConfirmationMismatch);
        }
        validate_request(&plan.request)?;
        if let Some(observation) = self
            .transport
            .inspect_repository(&plan.request.owner, &plan.request.name)?
        {
            validate_observation(&plan.request, &observation)?;
            return Ok(RepositoryApplyResult {
                disposition: RepositoryApplyDisposition::Connected,
                observation,
            });
        }
        let observation = self
            .transport
            .create_private_repository(&plan.request.owner, &plan.request.name)?;
        validate_observation(&plan.request, &observation)?;
        Ok(RepositoryApplyResult {
            disposition: RepositoryApplyDisposition::Created,
            observation,
        })
    }
}

fn validate_request(request: &GitHubRepositoryRequest) -> Result<(), GitHubOnboardingError> {
    if request.owner.trim().is_empty()
        || request.name.trim().is_empty()
        || request.owner_node_id.trim().is_empty()
    {
        return Err(GitHubOnboardingError::InvalidRequest);
    }
    Ok(())
}

fn validate_observation(
    request: &GitHubRepositoryRequest,
    observation: &GitHubRepositoryObservation,
) -> Result<(), GitHubOnboardingError> {
    if request.role == RepositoryRole::PersonalContext
        && observation.account_node_id != request.owner_node_id
    {
        return Err(GitHubOnboardingError::PersonalOwnerMismatch);
    }
    if observation.owner != request.owner || observation.name != request.name {
        return Err(GitHubOnboardingError::RepositoryIdentityMismatch);
    }
    if !observation.private {
        return Err(GitHubOnboardingError::RepositoryMustBePrivate);
    }
    if observation.repository_node_id.trim().is_empty() {
        return Err(GitHubOnboardingError::MissingRepositoryNodeId);
    }
    Ok(())
}

fn placeholder_digest() -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", "0".repeat(64))).expect("static digest")
}

#[derive(Debug, Error)]
pub enum GitHubOnboardingError {
    #[error("GitHub repository request is incomplete")]
    InvalidRequest,
    #[error("GitHub onboarding confirmation does not match the current plan")]
    ConfirmationMismatch,
    #[error("personal-context repository owner does not match the authenticated user")]
    PersonalOwnerMismatch,
    #[error("GitHub repository identity changed after planning")]
    RepositoryIdentityMismatch,
    #[error("CommonKit context repositories must be private")]
    RepositoryMustBePrivate,
    #[error("GitHub repository observation is missing its immutable node ID")]
    MissingRepositoryNodeId,
    #[error("GitHub transport failed: {0}")]
    Transport(String),
    #[error(transparent)]
    Contract(#[from] ContractError),
}
