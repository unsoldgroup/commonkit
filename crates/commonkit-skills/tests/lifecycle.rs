use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::{
    CandidateState, EvaluationCaseManifest, EvaluationMetric, EvaluationOutcome, EvaluationReceipt,
    GitRevision, HarnessLock, ModelLock, OptimizationLimits, PortableSourcePath, ProviderLock,
    ProviderSource, SchemaVersion, Sha256Digest, SkillDescriptor, SkillEvaluationSuite,
    SkillLifecycle, SkillOptimizationManifest, StableId,
};
use commonkit_reconcile::SkillPromotionAuthority;
use commonkit_skills::{Approval, FakeOptimizer, SkillEngine};

static NONCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "commonkit-skills-test-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("test directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

#[test]
fn inventory_uses_only_repo_bundled_agents_skill_source() {
    let directory = TestDirectory::new();
    let repository = directory.path().join("repository");
    let state = directory.path().join("state");
    fs::create_dir_all(repository.join(".agents/skills/review")).expect("canonical skill");
    fs::create_dir_all(repository.join("skills/legacy")).expect("legacy skill");
    fs::write(
        repository.join(".agents/skills/review/SKILL.md"),
        "# Canonical\n",
    )
    .expect("canonical");
    fs::write(repository.join("skills/legacy/SKILL.md"), "# Legacy\n").expect("legacy");
    let inventory = SkillEngine::open(&repository, &state)
        .expect("engine")
        .inventory()
        .expect("inventory");
    assert_eq!(inventory.len(), 1);
    assert_eq!(
        inventory[0].source_path.as_str(),
        ".agents/skills/review/SKILL.md"
    );
    fs::remove_dir_all(directory.path()).expect("cleanup");
}

#[test]
fn plugin_inventory_uses_the_explicit_plugin_skill_root() {
    let directory = TestDirectory::new();
    let repository = directory.path().join("repository");
    let state = directory.path().join("state");
    fs::create_dir_all(repository.join("plugin/skills/review")).expect("plugin skill");
    fs::write(
        repository.join("plugin/skills/review/SKILL.md"),
        "# Plugin skill\n",
    )
    .expect("skill");

    let inventory = SkillEngine::open_plugin_repository(&repository, &state)
        .expect("engine")
        .inventory()
        .expect("inventory");

    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].source_path.as_str(), "plugin/skills/review/SKILL.md");
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}

fn digest(value: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", value.to_string().repeat(64))).expect("digest")
}

fn fixture(root: &std::path::Path) -> (SkillOptimizationManifest, SkillEvaluationSuite) {
    fs::create_dir_all(root.join(".agents/skills/review")).expect("skill directory");
    fs::write(
        root.join(".agents/skills/review/SKILL.md"),
        "# Review\n\nFind bugs.\n",
    )
    .expect("skill");
    fs::create_dir_all(root.join("policies")).expect("policies");
    let policy = b"{\"adoption\":\"review_required\"}\n";
    fs::write(root.join("policies/skill-optimization.policy.json"), policy).expect("policy");
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
                .arg(root)
                .args(args)
                .status()
                .expect("git")
                .success()
        );
    }
    let revision = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("head")
            .stdout,
    )
    .expect("utf8");
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
    let skill_bytes = fs::read(root.join(".agents/skills/review/SKILL.md")).expect("read skill");
    let manifest = SkillOptimizationManifest {
        schema_version: SchemaVersion(1),
        id: id("optimization-one"),
        skill: SkillDescriptor {
            id: id("review"),
            source_path: PortableSourcePath::parse(".agents/skills/review/SKILL.md").expect("path"),
            source_digest: commonkit_skills::digest_bytes(&skill_bytes).expect("digest"),
            package: Some(id("review-package")),
            targets: BTreeSet::from([id("codex")]),
            lifecycle: SkillLifecycle::Active,
        },
        suite_digest: suite.digest().expect("suite digest"),
        evidence_digests: Vec::new(),
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
        policy_digest: commonkit_skills::digest_bytes(policy).expect("policy digest"),
        repository_revision: GitRevision::parse(revision.trim()).expect("revision"),
    };
    (manifest, suite)
}

fn passing_receipt(harness: HarnessLock) -> EvaluationReceipt {
    EvaluationReceipt {
        schema_version: SchemaVersion(1),
        baseline_basis_points: 5_000,
        candidate_basis_points: 6_000,
        held_out_baseline_basis_points: 5_000,
        held_out_candidate_basis_points: 5_000,
        required_cases: BTreeMap::from([(id("held"), EvaluationOutcome::Passed)]),
        cost_micros: 0,
        harness,
        scorer_digest: digest('7'),
    }
}

#[test]
fn candidate_survives_restart_and_promotes_only_with_fresh_approval() {
    let directory = TestDirectory::new();
    let repository = directory.path().join("repository");
    let state = directory.path().join("state");
    let (manifest, suite) = fixture(&repository);
    let optimizer = FakeOptimizer::new(
        b"# Review\n\nFind correctness, security, and reliability bugs.\n".to_vec(),
        passing_receipt(suite.harness.clone()),
    );

    let engine = SkillEngine::open(&repository, &state).expect("engine");
    let candidate = engine
        .optimize(&manifest, &suite, &optimizer)
        .expect("candidate");
    assert_eq!(candidate.state, CandidateState::Approvable);
    drop(engine);

    let engine = SkillEngine::open(&repository, &state).expect("restart engine");
    let recovered = engine.candidate(&candidate.id).expect("load candidate");
    assert_eq!(recovered.candidate_digest, candidate.candidate_digest);

    let plan = engine
        .plan_promotion(
            &candidate.id,
            Approval {
                approver: id("al"),
                approved_at_unix_ms: 1,
                reason: "reviewed".into(),
            },
            manifest.repository_revision.clone(),
        )
        .expect("promotion plan");
    let receipt = engine.apply_promotion(&plan).expect("promotion");
    assert_eq!(receipt.state.to_string(), "promoted");
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md"))
            .expect("promoted skill"),
        "# Review\n\nFind correctness, security, and reliability bugs.\n"
    );
    let authenticated = engine
        .authenticate(&receipt.id)
        .expect("durable promotion authority");
    assert_eq!(authenticated.candidate_id, candidate.id);
    assert_eq!(authenticated.candidate_digest, candidate.candidate_digest);

    engine.rollback_promotion(&receipt).expect("rollback");
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md")).expect("rolled back"),
        "# Review\n\nFind bugs.\n"
    );
}

#[test]
fn stale_source_rejects_promotion_without_mutation() {
    let directory = TestDirectory::new();
    let repository = directory.path().join("repository");
    let state = directory.path().join("state");
    let (manifest, suite) = fixture(&repository);
    let optimizer = FakeOptimizer::new(
        b"# Review\n\nImproved.\n".to_vec(),
        passing_receipt(suite.harness.clone()),
    );
    let engine = SkillEngine::open(&repository, &state).expect("engine");
    let candidate = engine
        .optimize(&manifest, &suite, &optimizer)
        .expect("candidate");
    let plan = engine
        .plan_promotion(
            &candidate.id,
            Approval {
                approver: id("al"),
                approved_at_unix_ms: 1,
                reason: "reviewed".into(),
            },
            manifest.repository_revision.clone(),
        )
        .expect("plan");
    fs::write(
        repository.join(".agents/skills/review/SKILL.md"),
        "# Review\n\nConcurrent edit.\n",
    )
    .expect("concurrent edit");

    let error = engine.apply_promotion(&plan).expect_err("stale source");
    assert_eq!(error.to_string(), "promotion source changed after planning");
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md"))
            .expect("source retained"),
        "# Review\n\nConcurrent edit.\n"
    );
}

#[test]
fn changed_git_head_rejects_approved_plan_without_mutation() {
    let directory = TestDirectory::new();
    let repository = directory.path().join("repository");
    let state = directory.path().join("state");
    let (manifest, suite) = fixture(&repository);
    let engine = SkillEngine::open(&repository, &state).expect("engine");
    let candidate = engine
        .optimize(
            &manifest,
            &suite,
            &FakeOptimizer::new(
                b"# Review\n\nImproved.\n".to_vec(),
                passing_receipt(suite.harness.clone()),
            ),
        )
        .expect("candidate");
    let plan = engine
        .plan_promotion(
            &candidate.id,
            Approval {
                approver: id("al"),
                approved_at_unix_ms: 1,
                reason: "reviewed".into(),
            },
            manifest.repository_revision,
        )
        .expect("plan");
    fs::write(repository.join("unrelated.txt"), "new revision\n").expect("new file");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["add", "unrelated.txt"])
            .status()
            .expect("git add")
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["commit", "-qm", "advance head"])
            .status()
            .expect("git commit")
            .success()
    );

    let error = engine.apply_promotion(&plan).expect_err("stale head");
    assert_eq!(
        error.to_string(),
        "promotion repository revision or policy changed after approval"
    );
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md")).expect("source"),
        "# Review\n\nFind bugs.\n"
    );
}

#[test]
fn changed_policy_rejects_approved_plan_without_mutation() {
    let directory = TestDirectory::new();
    let repository = directory.path().join("repository");
    let state = directory.path().join("state");
    let (manifest, suite) = fixture(&repository);
    let engine = SkillEngine::open(&repository, &state).expect("engine");
    let candidate = engine
        .optimize(
            &manifest,
            &suite,
            &FakeOptimizer::new(
                b"# Review\n\nImproved.\n".to_vec(),
                passing_receipt(suite.harness.clone()),
            ),
        )
        .expect("candidate");
    let plan = engine
        .plan_promotion(
            &candidate.id,
            Approval {
                approver: id("al"),
                approved_at_unix_ms: 1,
                reason: "reviewed".into(),
            },
            manifest.repository_revision,
        )
        .expect("plan");
    fs::write(
        repository.join("policies/skill-optimization.policy.json"),
        b"{\"adoption\":\"deny\"}\n",
    )
    .expect("policy change");

    let error = engine.apply_promotion(&plan).expect_err("stale policy");
    assert_eq!(
        error.to_string(),
        "promotion repository revision or policy changed after approval"
    );
    assert_eq!(
        fs::read_to_string(repository.join(".agents/skills/review/SKILL.md")).expect("source"),
        "# Review\n\nFind bugs.\n"
    );
}
