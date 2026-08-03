use std::collections::BTreeSet;

use commonkit_contracts::portable_context::{
    AccessLevel, EvidenceState, GitHubOnboardingAttempt, GitHubOnboardingState, OwnerKind,
    PortableRecordEnvelope, PortableRecordKind, Principal, PrincipalKind, ProfileExtension,
    ProfileFieldDefinition, ProfileFieldType, RecordOwner, RecordScope, RepositoryAccessEvidence,
    RepositoryRole, core_profile_schema, portable_context_schema_registry,
};
use commonkit_contracts::{SchemaVersion, Sha256Digest, StableId};

#[test]
fn portable_context_registry_exposes_the_complete_v1_contract_family() {
    let actual = portable_context_schema_registry()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    let expected = [
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
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();

    assert_eq!(actual, expected);
    assert!(portable_context_schema_registry().iter().all(|entry| {
        entry.schema_id
            == format!(
                "https://schemas.commonkit.dev/v1/portable-context/{}.schema.json",
                entry.name
            )
    }));
}

#[test]
fn every_portable_context_registry_entry_generates_its_addressed_schema() {
    for entry in portable_context_schema_registry() {
        let schema = entry.schema_value().unwrap();
        assert_eq!(
            schema.get("$id").and_then(serde_json::Value::as_str),
            Some(entry.schema_id.as_str()),
            "{} schema ID",
            entry.name
        );
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&serde_json::Value::Bool(false)),
            "{} must be a closed top-level schema",
            entry.name
        );
    }
}

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn envelope(kind: PortableRecordKind) -> PortableRecordEnvelope {
    PortableRecordEnvelope {
        schema_version: SchemaVersion(1),
        kind,
        id: StableId::parse("record-1").unwrap(),
        owner: RecordOwner {
            kind: OwnerKind::User,
            id: StableId::parse("user-1").unwrap(),
        },
        scope: RecordScope {
            organization_id: StableId::parse("org-1").unwrap(),
            project_id: Some(StableId::parse("project-1").unwrap()),
        },
        generation: 2,
        parent_hashes: vec![digest('a'), digest('b')],
        payload_hash: digest('c'),
        signer_id: StableId::parse("device-1").unwrap(),
        created_at_unix_ms: 42,
        signature: "ed25519:signature".into(),
    }
}

#[test]
fn signed_record_digest_is_domain_separated_and_excludes_the_signature() {
    let mut grant = envelope(PortableRecordKind::ContextGrant);
    let original = grant.signing_digest().unwrap();
    grant.signature = "ed25519:replacement".into();
    assert_eq!(grant.signing_digest().unwrap(), original);

    let tombstone = envelope(PortableRecordKind::DeletionTombstone);
    assert_ne!(original, tombstone.signing_digest().unwrap());
}

#[test]
fn signed_record_rejects_noncanonical_parent_history() {
    let mut record = envelope(PortableRecordKind::ProfileRevision);
    record.parent_hashes.reverse();
    assert_eq!(
        record.validate().unwrap_err().to_string(),
        "portable record parent hashes must be unique and sorted"
    );

    record.parent_hashes = vec![digest('a'), digest('a')];
    assert_eq!(
        record.validate().unwrap_err().to_string(),
        "portable record parent hashes must be unique and sorted"
    );
}

#[test]
fn signed_history_requires_authorized_signers_and_monotonic_parent_generations() {
    use commonkit_contracts::portable_context::validate_signed_history;

    let mut root = envelope(PortableRecordKind::ProfileRevision);
    root.generation = 1;
    root.parent_hashes.clear();
    let root_hash = root.signing_digest().unwrap();
    let mut child = envelope(PortableRecordKind::ProfileRevision);
    child.id = StableId::parse("record-2").unwrap();
    child.parent_hashes = vec![root_hash];

    let authorized = [StableId::parse("device-1").unwrap()].into_iter().collect();
    let verify = |_: &StableId, _: &Sha256Digest, signature: &str| signature == "ed25519:signature";
    validate_signed_history(&[root.clone(), child.clone()], &authorized, verify).unwrap();

    child.generation = 1;
    assert_eq!(
        validate_signed_history(&[root.clone(), child], &authorized, verify)
            .unwrap_err()
            .to_string(),
        "portable record generation must be greater than every parent generation"
    );

    let unauthorized = BTreeSet::new();
    assert_eq!(
        validate_signed_history(&[root.clone()], &unauthorized, verify)
            .unwrap_err()
            .to_string(),
        "portable record signer is not authorized: device-1"
    );

    assert_eq!(
        validate_signed_history(&[root], &authorized, |_, _, _| false)
            .unwrap_err()
            .to_string(),
        "portable record signature is invalid"
    );
}

#[test]
fn repository_access_evidence_must_be_fresh_and_unrevoked() {
    let mut evidence = RepositoryAccessEvidence {
        schema_version: SchemaVersion(1),
        id: StableId::parse("access-1").unwrap(),
        principal: Principal {
            kind: PrincipalKind::User,
            provider: StableId::parse("github").unwrap(),
            subject_id: "MDQ6VXNlcjE=".into(),
            display_name: "alice".into(),
        },
        repository_role: RepositoryRole::PersonalContext,
        repository_node_id: "R_kgDOExample".into(),
        repository_owner_node_id: "MDQ6VXNlcjE=".into(),
        repository_name: "alice/commonkit-context".into(),
        private: true,
        access: AccessLevel::Admin,
        checked_at_unix_ms: 1_000,
        expires_at_unix_ms: 2_000,
        state: EvidenceState::Active,
    };

    evidence.validate_at(1_999).unwrap();
    assert_eq!(
        evidence.validate_at(2_000).unwrap_err().to_string(),
        "repository access evidence is expired"
    );
    evidence.expires_at_unix_ms = 3_000;
    evidence.state = EvidenceState::Revoked;
    assert_eq!(
        evidence.validate_at(2_000).unwrap_err().to_string(),
        "repository access evidence is revoked"
    );
}

#[test]
fn github_onboarding_attempt_allows_only_resumable_state_transitions() {
    let attempt = GitHubOnboardingAttempt {
        schema_version: SchemaVersion(1),
        id: StableId::parse("onboarding-1").unwrap(),
        repository_role: RepositoryRole::PersonalContext,
        account_node_id: "MDQ6VXNlcjE=".into(),
        repository_node_id: Some("R_kgDOExample".into()),
        plan_digest: digest('d'),
        confirmation_digest: digest('e'),
        idempotency_key: StableId::parse("attempt-1").unwrap(),
        registration_id: None,
        state: GitHubOnboardingState::RemoteCreated,
        failure_code: None,
        updated_at_unix_ms: 100,
    };

    assert!(attempt.can_transition_to(GitHubOnboardingState::Orphaned));
    assert!(attempt.can_transition_to(GitHubOnboardingState::Registering));
    assert!(!attempt.can_transition_to(GitHubOnboardingState::Planned));
    assert!(!attempt.can_transition_to(GitHubOnboardingState::Completed));

    let completed = GitHubOnboardingAttempt {
        registration_id: Some(StableId::parse("registration-1").unwrap()),
        state: GitHubOnboardingState::Completed,
        ..attempt
    };
    assert!(!completed.can_transition_to(GitHubOnboardingState::Registering));
    completed.validate().unwrap();
}

#[test]
fn core_profile_catalog_is_closed_and_extensions_reject_prohibited_profiling() {
    let core = core_profile_schema();
    assert_eq!(core.fields.len(), 38);
    assert_eq!(
        core.fields["identity.responsibilities"].field_type,
        ProfileFieldType::StringList
    );
    assert_eq!(
        core.fields["communication.tone"].field_type,
        ProfileFieldType::String
    );

    let serialized = serde_json::to_value(&core).unwrap();
    let mut unknown = serialized.as_object().unwrap().clone();
    unknown.insert("unexpected".into(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<commonkit_contracts::portable_context::ProfileSchema>(
            unknown.into()
        )
        .is_err()
    );

    let extension = ProfileExtension {
        schema_version: SchemaVersion(1),
        namespace: StableId::parse("acme").unwrap(),
        version: 1,
        fields: [(
            "health.diagnosis".into(),
            ProfileFieldDefinition {
                field_type: ProfileFieldType::String,
                description: "Medical diagnosis".into(),
                optional: true,
            },
        )]
        .into(),
    };
    assert_eq!(
        extension.validate().unwrap_err().to_string(),
        "profile extension field belongs to a prohibited category: health.diagnosis"
    );
}

#[test]
fn registry_uses_the_concrete_profile_contract_schema() {
    let entry = portable_context_schema_registry()
        .into_iter()
        .find(|entry| entry.name == "profile-schema")
        .unwrap();
    let schema = entry.schema_value().unwrap();
    let properties = schema["properties"].as_object().unwrap();

    assert!(properties.contains_key("fields"));
    assert!(properties.contains_key("version"));
    assert!(!properties.contains_key("payload"));
}

#[test]
fn no_catalog_entry_falls_back_to_an_untyped_payload() {
    let generic = portable_context_schema_registry()
        .into_iter()
        .filter_map(|entry| {
            let schema = entry.schema_value().unwrap();
            schema["properties"]
                .as_object()
                .is_some_and(|properties| properties.contains_key("payload"))
                .then_some(entry.name)
        })
        .collect::<Vec<_>>();

    assert!(generic.is_empty(), "generic schemas remain: {generic:?}");
}

#[test]
fn every_contract_has_explicit_version_compatibility_behavior() {
    use commonkit_contracts::portable_context::{CompatibilityAction, dispatch_schema_version};

    for entry in portable_context_schema_registry() {
        assert_eq!(
            dispatch_schema_version(entry.name, 1).unwrap(),
            CompatibilityAction::ReadCurrent
        );
        assert_eq!(
            dispatch_schema_version(entry.name, 0).unwrap(),
            CompatibilityAction::UpgradeFrom(0)
        );
        assert_eq!(
            dispatch_schema_version(entry.name, 2).unwrap(),
            CompatibilityAction::QuarantineNewer(2)
        );
    }

    assert_eq!(
        commonkit_contracts::portable_context::ensure_writable_schema_version(0)
            .unwrap_err()
            .to_string(),
        "refusing to write obsolete portable-context schema version 0"
    );
}

#[test]
fn unknown_profile_extension_namespaces_are_quarantined_without_loss() {
    use commonkit_contracts::portable_context::{LoadedProfileExtension, load_profile_extension};
    use std::collections::BTreeMap;

    let raw = serde_json::json!({
        "schemaVersion": 1,
        "namespace": "future-team",
        "version": 3,
        "fields": {
            "future.focus": {
                "fieldType": "string",
                "description": "Future field",
                "optional": true
            }
        }
    });
    let loaded = load_profile_extension(raw.clone(), &BTreeMap::new()).unwrap();
    assert_eq!(
        loaded,
        LoadedProfileExtension::Quarantined {
            namespace: StableId::parse("future-team").unwrap(),
            version: 3,
            raw,
        }
    );
    assert!(!loaded.is_publishable());
}
