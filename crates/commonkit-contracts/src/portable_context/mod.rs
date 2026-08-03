//! Versioned wire contracts for portable organization, project, and user context.

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

use crate::{ContractError, SchemaVersion, Sha256Digest, StableId, digest_domain_json};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PortableRecordKind {
    ContextGrant,
    ContextGrantRevocation,
    DeletionTombstone,
    DeviceAuthorization,
    DeviceCertificate,
    DeviceRevocation,
    ProfileRevision,
}

impl PortableRecordKind {
    fn signing_domain(self) -> &'static str {
        match self {
            Self::ContextGrant => "commonkit.portable-context.context-grant.v1",
            Self::ContextGrantRevocation => {
                "commonkit.portable-context.context-grant-revocation.v1"
            }
            Self::DeletionTombstone => "commonkit.portable-context.deletion-tombstone.v1",
            Self::DeviceAuthorization => "commonkit.portable-context.device-authorization.v1",
            Self::DeviceCertificate => "commonkit.portable-context.device-certificate.v1",
            Self::DeviceRevocation => "commonkit.portable-context.device-revocation.v1",
            Self::ProfileRevision => "commonkit.portable-context.profile-revision.v1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Organization,
    Project,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordOwner {
    pub kind: OwnerKind,
    pub id: StableId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordScope {
    pub organization_id: StableId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<StableId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    AgentRuntime,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Principal {
    pub kind: PrincipalKind,
    pub provider: StableId,
    /// Immutable provider identifier, not a mutable login or display name.
    pub subject_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryRole {
    OrganizationKit,
    ProjectContext,
    PersonalContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    Read,
    Write,
    Maintain,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Active,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryAccessEvidence {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub principal: Principal,
    pub repository_role: RepositoryRole,
    pub repository_node_id: String,
    pub repository_owner_node_id: String,
    pub repository_name: String,
    pub private: bool,
    pub access: AccessLevel,
    pub checked_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub state: EvidenceState,
}

impl RepositoryAccessEvidence {
    pub fn validate_at(&self, now_unix_ms: u64) -> Result<(), PortableContextError> {
        if self.state == EvidenceState::Revoked {
            return Err(PortableContextError::RevokedRepositoryEvidence);
        }
        if now_unix_ms >= self.expires_at_unix_ms {
            return Err(PortableContextError::ExpiredRepositoryEvidence);
        }
        if self.checked_at_unix_ms >= self.expires_at_unix_ms {
            return Err(PortableContextError::InvalidEvidenceWindow);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHubOnboardingState {
    Planned,
    RemoteCreated,
    Orphaned,
    Registering,
    Registered,
    Verifying,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubOnboardingAttempt {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub repository_role: RepositoryRole,
    pub account_node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_node_id: Option<String>,
    pub plan_digest: Sha256Digest,
    pub confirmation_digest: Sha256Digest,
    pub idempotency_key: StableId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_id: Option<StableId>,
    pub state: GitHubOnboardingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<StableId>,
    pub updated_at_unix_ms: u64,
}

impl GitHubOnboardingAttempt {
    pub fn can_transition_to(&self, next: GitHubOnboardingState) -> bool {
        use GitHubOnboardingState as State;
        matches!(
            (self.state, next),
            (
                State::Planned,
                State::RemoteCreated | State::Registering | State::Failed
            ) | (
                State::RemoteCreated,
                State::Orphaned | State::Registering | State::Failed
            ) | (State::Orphaned, State::Registering | State::Failed)
                | (
                    State::Registering,
                    State::Registered | State::Orphaned | State::Failed
                )
                | (State::Registered, State::Verifying | State::Failed)
                | (State::Verifying, State::Completed | State::Failed)
        )
    }

    pub fn validate(&self) -> Result<(), PortableContextError> {
        if self.account_node_id.trim().is_empty() {
            return Err(PortableContextError::MissingProviderNodeId("account"));
        }
        if matches!(
            self.state,
            GitHubOnboardingState::RemoteCreated
                | GitHubOnboardingState::Orphaned
                | GitHubOnboardingState::Registering
                | GitHubOnboardingState::Registered
                | GitHubOnboardingState::Verifying
                | GitHubOnboardingState::Completed
        ) && self.repository_node_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(PortableContextError::MissingProviderNodeId("repository"));
        }
        if self.state == GitHubOnboardingState::Completed && self.registration_id.is_none() {
            return Err(PortableContextError::MissingRegistrationId);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableRecordEnvelope {
    pub schema_version: SchemaVersion,
    pub kind: PortableRecordKind,
    pub id: StableId,
    pub owner: RecordOwner,
    pub scope: RecordScope,
    pub generation: u64,
    pub parent_hashes: Vec<Sha256Digest>,
    pub payload_hash: Sha256Digest,
    pub signer_id: StableId,
    pub created_at_unix_ms: u64,
    pub signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SigningPayload<'a> {
    schema_version: SchemaVersion,
    kind: PortableRecordKind,
    id: &'a StableId,
    owner: &'a RecordOwner,
    scope: &'a RecordScope,
    generation: u64,
    parent_hashes: &'a [Sha256Digest],
    payload_hash: &'a Sha256Digest,
    signer_id: &'a StableId,
    created_at_unix_ms: u64,
}

impl PortableRecordEnvelope {
    pub fn signing_digest(&self) -> Result<Sha256Digest, ContractError> {
        digest_domain_json(
            self.kind.signing_domain(),
            &SigningPayload {
                schema_version: self.schema_version,
                kind: self.kind,
                id: &self.id,
                owner: &self.owner,
                scope: &self.scope,
                generation: self.generation,
                parent_hashes: &self.parent_hashes,
                payload_hash: &self.payload_hash,
                signer_id: &self.signer_id,
                created_at_unix_ms: self.created_at_unix_ms,
            },
        )
    }

    pub fn validate(&self) -> Result<(), PortableContextError> {
        if self.generation == 0 {
            return Err(PortableContextError::ZeroGeneration);
        }
        if self.signature.trim().is_empty() {
            return Err(PortableContextError::MissingSignature);
        }
        if self.parent_hashes.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(PortableContextError::NoncanonicalParents);
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PortableContextError {
    #[error("portable record generation must be positive")]
    ZeroGeneration,
    #[error("portable record signature must be present")]
    MissingSignature,
    #[error("portable record parent hashes must be unique and sorted")]
    NoncanonicalParents,
    #[error("repository access evidence is expired")]
    ExpiredRepositoryEvidence,
    #[error("repository access evidence is revoked")]
    RevokedRepositoryEvidence,
    #[error("repository access evidence has an invalid freshness window")]
    InvalidEvidenceWindow,
    #[error("GitHub onboarding is missing immutable {0} node ID")]
    MissingProviderNodeId(&'static str),
    #[error("completed GitHub onboarding is missing its registration ID")]
    MissingRegistrationId,
    #[error("profile extension field belongs to a prohibited category: {0}")]
    ProhibitedProfileField(String),
    #[error("unknown portable-context contract kind: {0}")]
    UnknownContractKind(String),
    #[error("refusing to write obsolete portable-context schema version {0}")]
    DowngradeRefused(u32),
    #[error("portable record signer is not authorized: {0}")]
    UnauthorizedSigner(StableId),
    #[error("portable record signature is invalid")]
    InvalidSignature,
    #[error("portable record references an unknown or non-prior parent")]
    UnknownParent,
    #[error("portable record generation must be greater than every parent generation")]
    NonmonotonicGeneration,
    #[error("profile extension fields must be optional: {0}")]
    RequiredExtensionField(String),
    #[error("profile extension document is invalid")]
    InvalidProfileExtension,
}

pub fn validate_signed_history<F>(
    records: &[PortableRecordEnvelope],
    authorized_signers: &BTreeSet<StableId>,
    verify_signature: F,
) -> Result<(), PortableContextError>
where
    F: Fn(&StableId, &Sha256Digest, &str) -> bool,
{
    let mut generations = BTreeMap::new();
    for record in records {
        record.validate()?;
        if !authorized_signers.contains(&record.signer_id) {
            return Err(PortableContextError::UnauthorizedSigner(
                record.signer_id.clone(),
            ));
        }
        for parent in &record.parent_hashes {
            let parent_generation = generations
                .get(parent)
                .ok_or(PortableContextError::UnknownParent)?;
            if record.generation <= *parent_generation {
                return Err(PortableContextError::NonmonotonicGeneration);
            }
        }
        let digest = record
            .signing_digest()
            .map_err(|_| PortableContextError::UnknownParent)?;
        if !verify_signature(&record.signer_id, &digest, &record.signature) {
            return Err(PortableContextError::InvalidSignature);
        }
        generations.insert(digest, record.generation);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityAction {
    UpgradeFrom(u32),
    ReadCurrent,
    QuarantineNewer(u32),
}

pub fn dispatch_schema_version(
    contract_name: &str,
    version: u32,
) -> Result<CompatibilityAction, PortableContextError> {
    if !CONTRACT_NAMES.contains(&contract_name) {
        return Err(PortableContextError::UnknownContractKind(
            contract_name.to_owned(),
        ));
    }
    Ok(match version.cmp(&1) {
        std::cmp::Ordering::Less => CompatibilityAction::UpgradeFrom(version),
        std::cmp::Ordering::Equal => CompatibilityAction::ReadCurrent,
        std::cmp::Ordering::Greater => CompatibilityAction::QuarantineNewer(version),
    })
}

pub fn ensure_writable_schema_version(version: u32) -> Result<(), PortableContextError> {
    if version < 1 {
        return Err(PortableContextError::DowngradeRefused(version));
    }
    Ok(())
}

macro_rules! define_portable_contracts {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            pub struct $name {
                pub schema_version: SchemaVersion,
                pub id: StableId,
                pub owner: RecordOwner,
                pub scope: RecordScope,
                pub content_hash: Sha256Digest,
                pub created_at_unix_ms: u64,
                /// Opaque source-record identifiers; never embeds source content.
                pub provenance: Vec<StableId>,
            }
        )+
    };
}

// These source-record contracts intentionally share the universal portable
// metadata shape. Behavior-specific projections are modeled separately and
// can evolve without weakening the closed top-level wire format.
define_portable_contracts!(
    AgentTrustClass,
    ContextDescriptor,
    ContextGrant,
    ContextGrantRevocation,
    ContextReceipt,
    ContextReceiptEvent,
    ContextSection,
    DeletionTombstone,
    DeviceAuthorization,
    DeviceCertificate,
    DeviceRevocation,
    DocumentationArtifact,
    DocumentationConflict,
    DocumentationRevisionCandidate,
    EncryptedProfileDocument,
    GitHubOrphanReceipt,
    OrganizationMembership,
    OrganizationOnboardingRecord,
    OrganizationRegistration,
    ProfileConflict,
    ProfileDraft,
    ProfileRevision,
    ProfileRevisionProposal,
    ProfileSigningAuthority,
    ProjectContextMap,
    ProjectRegistration,
    ReceiptView,
    RecoveryRecipient,
    RoleBinding,
    RuntimeAttestation,
    RuntimeSessionPrincipal,
    TaskContextBrief,
    TaskContextRequest,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFieldType {
    String,
    StringList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileFieldDefinition {
    pub field_type: ProfileFieldType,
    pub description: String,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSchema {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub version: u32,
    pub fields: BTreeMap<String, ProfileFieldDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileExtension {
    pub schema_version: SchemaVersion,
    pub namespace: StableId,
    pub version: u32,
    pub fields: BTreeMap<String, ProfileFieldDefinition>,
}

impl ProfileExtension {
    pub fn validate(&self) -> Result<(), PortableContextError> {
        const PROHIBITED_PREFIXES: &[&str] = &[
            "demographic.",
            "family.",
            "financial.",
            "health.",
            "lifestyle.",
        ];
        if let Some(name) = self.fields.keys().find(|name| {
            PROHIBITED_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        }) {
            return Err(PortableContextError::ProhibitedProfileField(name.clone()));
        }
        if let Some(name) = self
            .fields
            .iter()
            .find_map(|(name, field)| (!field.optional).then_some(name))
        {
            return Err(PortableContextError::RequiredExtensionField(name.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadedProfileExtension {
    Known(ProfileExtension),
    Quarantined {
        namespace: StableId,
        version: u32,
        raw: Value,
    },
}

impl LoadedProfileExtension {
    pub fn is_publishable(&self) -> bool {
        matches!(self, Self::Known(_))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionIdentity {
    namespace: StableId,
    version: u32,
}

pub fn load_profile_extension(
    raw: Value,
    supported_namespaces: &BTreeMap<StableId, u32>,
) -> Result<LoadedProfileExtension, PortableContextError> {
    let identity: ExtensionIdentity = serde_json::from_value(raw.clone())
        .map_err(|_| PortableContextError::InvalidProfileExtension)?;
    if supported_namespaces.get(&identity.namespace) != Some(&identity.version) {
        return Ok(LoadedProfileExtension::Quarantined {
            namespace: identity.namespace,
            version: identity.version,
            raw,
        });
    }
    let extension: ProfileExtension =
        serde_json::from_value(raw).map_err(|_| PortableContextError::InvalidProfileExtension)?;
    extension.validate()?;
    Ok(LoadedProfileExtension::Known(extension))
}

/// The fixed v1 work-profile catalog. All fields are optional so an interview can
/// be resumed or selectively answered without inventing personal information.
pub const CORE_PROFILE_FIELD_ORDER: &[&str] = &[
    "identity.display_name",
    "identity.pronouns",
    "identity.role",
    "identity.responsibilities",
    "identity.expertise",
    "communication.tone",
    "communication.detail_level",
    "communication.preferred_channels",
    "communication.async_expectations",
    "collaboration.working_style",
    "collaboration.handoff_preferences",
    "collaboration.meeting_preferences",
    "collaboration.escalation_preferences",
    "feedback.preferred_style",
    "feedback.correction_preferences",
    "feedback.praise_preferences",
    "decisions.decision_style",
    "decisions.evidence_expectations",
    "decisions.risk_tolerance",
    "decisions.approval_thresholds",
    "planning.planning_horizon",
    "planning.task_breakdown",
    "planning.status_update_preferences",
    "planning.definition_of_done",
    "technical.languages",
    "technical.frameworks",
    "technical.package_managers",
    "technical.tool_preferences",
    "technical.code_review_preferences",
    "accessibility.requested_accommodations",
    "accessibility.presentation_preferences",
    "boundaries.do_not_do",
    "boundaries.ask_before",
    "boundaries.availability_notes",
    "agents.response_style",
    "agents.autonomy_preferences",
    "agents.clarification_preferences",
    "agents.memory_preferences",
];

pub fn core_profile_schema() -> ProfileSchema {
    const STRING_FIELDS: &[&str] = &[
        "identity.display_name",
        "identity.pronouns",
        "identity.role",
        "communication.tone",
        "communication.detail_level",
        "communication.async_expectations",
        "collaboration.working_style",
        "collaboration.handoff_preferences",
        "collaboration.meeting_preferences",
        "collaboration.escalation_preferences",
        "feedback.preferred_style",
        "feedback.correction_preferences",
        "feedback.praise_preferences",
        "decisions.decision_style",
        "decisions.evidence_expectations",
        "decisions.risk_tolerance",
        "decisions.approval_thresholds",
        "planning.planning_horizon",
        "planning.task_breakdown",
        "planning.status_update_preferences",
        "planning.definition_of_done",
        "technical.code_review_preferences",
        "agents.response_style",
        "agents.autonomy_preferences",
        "agents.clarification_preferences",
        "agents.memory_preferences",
        "boundaries.availability_notes",
    ];
    const LIST_FIELDS: &[&str] = &[
        "identity.responsibilities",
        "identity.expertise",
        "communication.preferred_channels",
        "technical.languages",
        "technical.frameworks",
        "technical.package_managers",
        "technical.tool_preferences",
        "accessibility.requested_accommodations",
        "accessibility.presentation_preferences",
        "boundaries.do_not_do",
        "boundaries.ask_before",
    ];

    let fields = STRING_FIELDS
        .iter()
        .map(|name| (*name, ProfileFieldType::String))
        .chain(
            LIST_FIELDS
                .iter()
                .map(|name| (*name, ProfileFieldType::StringList)),
        )
        .map(|(name, field_type)| {
            (
                name.to_owned(),
                ProfileFieldDefinition {
                    field_type,
                    description: name.replace(['.', '_'], " "),
                    optional: true,
                },
            )
        })
        .collect();

    ProfileSchema {
        schema_version: SchemaVersion(1),
        id: StableId::parse("commonkit-work-profile").expect("static stable ID"),
        version: 1,
        fields,
    }
}

/// One public schema in the portable-context contract family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaRegistryEntry {
    pub name: &'static str,
    pub schema_id: String,
}

impl SchemaRegistryEntry {
    pub fn schema_value(&self) -> Result<Value, ContractError> {
        let mut schema = match self.name {
            "agent-trust-class" => schema_json::<AgentTrustClass>()?,
            "context-descriptor" => schema_json::<ContextDescriptor>()?,
            "context-grant" => schema_json::<ContextGrant>()?,
            "context-grant-revocation" => schema_json::<ContextGrantRevocation>()?,
            "context-receipt" => schema_json::<ContextReceipt>()?,
            "context-receipt-event" => schema_json::<ContextReceiptEvent>()?,
            "context-section" => schema_json::<ContextSection>()?,
            "deletion-tombstone" => schema_json::<DeletionTombstone>()?,
            "device-authorization" => schema_json::<DeviceAuthorization>()?,
            "device-certificate" => schema_json::<DeviceCertificate>()?,
            "device-revocation" => schema_json::<DeviceRevocation>()?,
            "documentation-artifact" => schema_json::<DocumentationArtifact>()?,
            "documentation-conflict" => schema_json::<DocumentationConflict>()?,
            "documentation-revision-candidate" => schema_json::<DocumentationRevisionCandidate>()?,
            "encrypted-profile-document" => schema_json::<EncryptedProfileDocument>()?,
            "github-onboarding-attempt" => schema_json::<GitHubOnboardingAttempt>()?,
            "github-orphan-receipt" => schema_json::<GitHubOrphanReceipt>()?,
            "organization-membership" => schema_json::<OrganizationMembership>()?,
            "organization-onboarding-record" => schema_json::<OrganizationOnboardingRecord>()?,
            "organization-registration" => schema_json::<OrganizationRegistration>()?,
            "principal" => schema_json::<Principal>()?,
            "profile-conflict" => schema_json::<ProfileConflict>()?,
            "profile-draft" => schema_json::<ProfileDraft>()?,
            "profile-extension" => schema_json::<ProfileExtension>()?,
            "profile-revision" => schema_json::<ProfileRevision>()?,
            "profile-revision-proposal" => schema_json::<ProfileRevisionProposal>()?,
            "profile-schema" => schema_json::<ProfileSchema>()?,
            "profile-signing-authority" => schema_json::<ProfileSigningAuthority>()?,
            "project-context-map" => schema_json::<ProjectContextMap>()?,
            "project-registration" => schema_json::<ProjectRegistration>()?,
            "receipt-view" => schema_json::<ReceiptView>()?,
            "recovery-recipient" => schema_json::<RecoveryRecipient>()?,
            "repository-access-evidence" => schema_json::<RepositoryAccessEvidence>()?,
            "role-binding" => schema_json::<RoleBinding>()?,
            "runtime-attestation" => schema_json::<RuntimeAttestation>()?,
            "runtime-session-principal" => schema_json::<RuntimeSessionPrincipal>()?,
            "task-context-brief" => schema_json::<TaskContextBrief>()?,
            "task-context-request" => schema_json::<TaskContextRequest>()?,
            _ => return Err(ContractError::SchemaGeneration),
        };
        let object = schema
            .as_object_mut()
            .ok_or(ContractError::SchemaGeneration)?;
        object.insert("$id".into(), Value::String(self.schema_id.clone()));
        object.insert("title".into(), Value::String(self.name.into()));
        Ok(schema)
    }
}

fn schema_json<T: JsonSchema>() -> Result<Value, ContractError> {
    serde_json::to_value(schema_for!(T)).map_err(|_| ContractError::SchemaGeneration)
}

const CONTRACT_NAMES: &[&str] = &[
    "agent-trust-class",
    "context-descriptor",
    "context-grant",
    "context-grant-revocation",
    "context-receipt",
    "context-receipt-event",
    "context-section",
    "deletion-tombstone",
    "device-authorization",
    "device-certificate",
    "device-revocation",
    "documentation-artifact",
    "documentation-conflict",
    "documentation-revision-candidate",
    "encrypted-profile-document",
    "github-onboarding-attempt",
    "github-orphan-receipt",
    "organization-membership",
    "organization-onboarding-record",
    "organization-registration",
    "principal",
    "profile-conflict",
    "profile-draft",
    "profile-extension",
    "profile-revision",
    "profile-revision-proposal",
    "profile-schema",
    "profile-signing-authority",
    "project-context-map",
    "project-registration",
    "receipt-view",
    "recovery-recipient",
    "repository-access-evidence",
    "role-binding",
    "runtime-attestation",
    "runtime-session-principal",
    "task-context-brief",
    "task-context-request",
];

/// Returns the complete v1 portable-context schema catalog in stable order.
pub fn portable_context_schema_registry() -> Vec<SchemaRegistryEntry> {
    CONTRACT_NAMES
        .iter()
        .map(|name| SchemaRegistryEntry {
            name,
            schema_id: format!(
                "https://schemas.commonkit.dev/v1/portable-context/{name}.schema.json"
            ),
        })
        .collect()
}
