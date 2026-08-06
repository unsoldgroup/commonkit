use std::collections::{BTreeMap, BTreeSet};

use commonkit_contracts::{
    CandidateState, ContentSensitivity, EvaluationCaseManifest, EvaluationMetric,
    EvaluationOutcome, EvaluationReceipt, GitRevision, HarnessLock, ModelLock, OptimizationLimits,
    PortableSourcePath, ProviderLock, ProviderSource, SchemaVersion, Sha256Digest, SkillCandidate,
    SkillDescriptor, SkillEvaluationSuite, SkillLifecycle, SkillOptimizationManifest, StableId,
};

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("stable id")
}

fn digest(value: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", value.to_string().repeat(64))).expect("digest")
}

fn cases(ids: &[&str], digest: char) -> EvaluationCaseManifest {
    EvaluationCaseManifest {
        content_digest: self::digest(digest),
        case_ids: ids.iter().map(|value| id(value)).collect(),
    }
}

fn suite() -> SkillEvaluationSuite {
    SkillEvaluationSuite {
        schema_version: SchemaVersion(1),
        id: id("review-suite"),
        skill_id: id("review"),
        train: cases(&["train-one"], '1'),
        validation: cases(&["validation-one"], '2'),
        held_out: cases(&["held-one"], '3'),
        rubric_digest: digest('4'),
        harness: HarnessLock {
            kind: id("codex-exec"),
            version: "0.1.0".into(),
            environment_digest: digest('5'),
        },
        metric: EvaluationMetric {
            id: id("accuracy"),
            minimum_improvement_basis_points: 100,
            maximum_held_out_regression_basis_points: 0,
            required_case_ids: BTreeSet::from([id("held-one")]),
        },
    }
}

#[test]
fn optimization_contracts_are_validated_and_content_addressed() {
    let suite = suite();
    suite.validate().expect("disjoint suite");

    let manifest = SkillOptimizationManifest {
        schema_version: SchemaVersion(1),
        id: id("opt-review-1"),
        skill: SkillDescriptor {
            id: id("review"),
            source_path: PortableSourcePath::parse("skills/review/SKILL.md").expect("path"),
            source_digest: digest('6'),
            package: Some(id("review-package")),
            targets: BTreeSet::from([id("codex"), id("claude")]),
            lifecycle: SkillLifecycle::Active,
        },
        suite_digest: suite.digest().expect("suite digest"),
        evidence_digests: vec![digest('7')],
        provider: ProviderLock {
            id: id("skillopt"),
            version: "0.2.0".into(),
            adapter_contract: id("skillopt-sleep-v1"),
            source: ProviderSource::Pypi,
            package_digest: digest('f'),
            capabilities: BTreeSet::from([
                id("reviewed-tasks"),
                id("staged-skill"),
                id("json-report"),
            ]),
            source_revision: Some(GitRevision::parse("a".repeat(40)).expect("revision")),
        },
        optimizer: ModelLock {
            provider: id("openai"),
            model: "gpt-5.5-2026-07-01".into(),
        },
        target: ModelLock {
            provider: id("openai"),
            model: "gpt-5.5-2026-07-01".into(),
        },
        limits: OptimizationLimits {
            maximum_cases: 20,
            maximum_edits: 5,
            timeout_seconds: 900,
            maximum_cost_micros: 2_000_000,
        },
        policy_digest: digest('8'),
        repository_revision: GitRevision::parse("b".repeat(40)).expect("revision"),
    };
    manifest.validate().expect("valid manifest");

    let evaluation = EvaluationReceipt {
        schema_version: SchemaVersion(1),
        baseline_basis_points: 7_000,
        candidate_basis_points: 7_500,
        held_out_baseline_basis_points: 7_000,
        held_out_candidate_basis_points: 7_000,
        required_cases: BTreeMap::from([(id("held-one"), EvaluationOutcome::Passed)]),
        cost_micros: 500_000,
        harness: suite.harness.clone(),
        scorer_digest: digest('9'),
    };
    let candidate = SkillCandidate {
        schema_version: SchemaVersion(1),
        id: id("candidate-one"),
        manifest_digest: manifest.digest().expect("manifest digest"),
        parent_skill_digest: manifest.skill.source_digest.clone(),
        provider: manifest.provider.clone(),
        optimizer: manifest.optimizer.clone(),
        target: manifest.target.clone(),
        configuration_digest: commonkit_contracts::digest_domain_json(
            "commonkit.skillopt-configuration.v1",
            &manifest.limits,
        )
        .expect("config digest"),
        input_digests: vec![
            manifest.skill.source_digest.clone(),
            manifest.suite_digest.clone(),
            manifest.policy_digest.clone(),
            digest('7'),
        ],
        candidate_digest: digest('c'),
        candidate_bytes: 512,
        candidate_sensitivity: ContentSensitivity::Portable,
        patch_digest: digest('d'),
        evaluation,
        optimization_history_digest: digest('e'),
        policy_passed: true,
        state: CandidateState::Approvable,
    };

    candidate
        .validate_against(&manifest, &suite)
        .expect("candidate satisfies gates");
    assert_eq!(
        manifest.digest().expect("digest"),
        manifest.digest().expect("same digest")
    );
}

#[test]
fn held_out_cases_cannot_overlap_optimizer_visible_splits() {
    let mut suite = suite();
    suite.held_out.case_ids = BTreeSet::from([id("train-one")]);

    assert_eq!(
        suite.validate().expect_err("overlap must fail").to_string(),
        "evaluation case train-one appears in more than one split"
    );
}
