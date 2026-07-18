#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::{
    EvaluationCaseManifest, EvaluationMetric, HarnessLock, OptimizationLimits, PortableSourcePath,
    ProviderLock, ProviderSource, SchemaVersion, Sha256Digest, SkillDescriptor,
    SkillEvaluationSuite, SkillLifecycle, SkillOptimizationManifest, StableId,
};
use commonkit_skills::{
    ProviderCheck, SkillOptBackend, SkillOptProviderConfig, SkillOptSleepOptimizer, SkillOptimizer,
};

static NONCE: AtomicU64 = AtomicU64::new(0);
fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}
fn digest(value: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", value.to_string().repeat(64))).expect("digest")
}

#[test]
fn provider_is_version_pinned_staged_and_never_adopts_live_source() {
    let root = std::env::temp_dir().join(format!(
        "commonkit-provider-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let bin = root.join("tool/bin");
    fs::create_dir_all(&bin).expect("bin");
    let python = bin.join("python");
    fs::write(&python, "#!/bin/sh\nprintf '0.2.0\\n'\n").expect("python probe");
    fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).expect("mode");
    let executable = bin.join("skillopt-sleep");
    fs::write(&executable, r#"#!/bin/sh
project=''
skill=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--project" ]; then project="$2"; shift 2
  elif [ "$1" = "--target-skill-path" ]; then skill="$2"; shift 2
  else shift; fi
done
mkdir -p "$project/.skillopt-sleep/staging/run-1"
printf '# Review\n\nImproved safely.\n' > "$project/.skillopt-sleep/staging/run-1/proposed_SKILL.md"
printf '{"live_skill_path":"%s","live_memory_path":"","has_skill":true,"has_memory":false,"accepted":true}' "$skill" > "$project/.skillopt-sleep/staging/run-1/manifest.json"
printf '{"night":1,"accepted":true,"gate_action":"accept","no_edits_reason":"","baseline":0.5,"candidate":0.7,"n_tasks":1,"n_sessions":0,"n_accepted_edits":1,"n_rejected_edits":0,"edits":[{"target":"skill","op":"add","content":"safe","anchor":"","rationale":"fixture"}],"rejected_edits":[],"notes":[],"staging_dir":"%s/.skillopt-sleep/staging/run-1","adopted":[],"tasks_file":"reviewed-tasks.json","tasks_reviewed":true}' "$project"
"#).expect("provider");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("mode");
    let provider_lock = ProviderLock {
        id: id("skillopt"),
        version: "0.2.0".into(),
        adapter_contract: id("skillopt-sleep-v1"),
        source: ProviderSource::Pypi,
        package_digest: digest('8'),
        capabilities: BTreeSet::from([id("reviewed-tasks"), id("staged-skill"), id("json-report")]),
        source_revision: None,
    };
    fs::write(
        root.join("tool/commonkit-provider-lock.json"),
        serde_json::to_vec(&ProviderCheck {
            provider: provider_lock.id.clone(),
            version: provider_lock.version.clone(),
            adapter_contract: provider_lock.adapter_contract.clone(),
            source: provider_lock.source,
            package_digest: provider_lock.package_digest.clone(),
            capabilities: provider_lock.capabilities.clone(),
            compatible: true,
        })
        .expect("marker"),
    )
    .expect("marker");
    let tasks = root.join("tasks.json");
    let corpus = root.join("corpus");
    fs::create_dir_all(&corpus).expect("corpus");
    let harness = root.join("harness");
    let suite = SkillEvaluationSuite {
        schema_version: SchemaVersion(1),
        id: id("suite"),
        skill_id: id("review"),
        train: EvaluationCaseManifest {
            content_digest: digest('1'),
            case_ids: BTreeSet::from([id("train")]),
        },
        validation: EvaluationCaseManifest {
            content_digest: digest('2'),
            case_ids: BTreeSet::from([id("valid")]),
        },
        held_out: EvaluationCaseManifest {
            content_digest: digest('3'),
            case_ids: BTreeSet::from([id("held")]),
        },
        rubric_digest: digest('4'),
        harness: HarnessLock {
            kind: id("skillopt-sleep"),
            version: "0.2.0".into(),
            environment_digest: digest('5'),
        },
        metric: EvaluationMetric {
            id: id("accuracy"),
            minimum_improvement_basis_points: 100,
            maximum_held_out_regression_basis_points: 0,
            required_case_ids: BTreeSet::from([id("held")]),
        },
    };
    let manifest = SkillOptimizationManifest {
        schema_version: SchemaVersion(1),
        id: id("run"),
        skill: SkillDescriptor {
            id: id("review"),
            source_path: PortableSourcePath::parse(".agents/skills/review/SKILL.md").expect("path"),
            source_digest: digest('6'),
            package: None,
            targets: BTreeSet::from([id("codex")]),
            lifecycle: SkillLifecycle::Active,
        },
        suite_digest: suite.digest().expect("suite"),
        evidence_digests: vec![],
        provider: provider_lock.clone(),
        optimizer: commonkit_contracts::ModelLock {
            provider: id("skillopt"),
            model: "mock".into(),
        },
        target: commonkit_contracts::ModelLock {
            provider: id("skillopt"),
            model: "mock".into(),
        },
        limits: OptimizationLimits {
            maximum_cases: 3,
            maximum_edits: 2,
            timeout_seconds: 5,
            maximum_cost_micros: 1,
        },
        policy_digest: digest('7'),
        repository_revision: commonkit_contracts::GitRevision::parse("a".repeat(40))
            .expect("revision"),
    };

    let original = b"# Review\n\nOriginal.\n";
    let candidate = b"# Review\n\nImproved safely.\n";
    let suite_digest = suite.digest().expect("suite digest");
    fs::write(
        &tasks,
        serde_json::to_vec(&serde_json::json!({
            "format": "skillopt_sleep.tasks.v1",
            "reviewed": true,
            "commonkit": {
                "skillId": "review",
                "campaignId": "suite",
                "suiteDigest": suite_digest,
                "trainDigest": suite.train.content_digest.clone(),
                "validationDigest": suite.validation.content_digest.clone(),
            },
            "tasks": [
                {"id":"train","prompt":"train fixture"},
                {"id":"valid","prompt":"validation fixture"}
            ]
        }))
        .expect("tasks json"),
    )
    .expect("tasks");
    let harness_report = serde_json::json!({
        "schemaVersion": 1,
        "suiteDigest": suite_digest,
        "skillId": "review",
        "baselineDigest": commonkit_skills::digest_bytes(original).expect("baseline digest"),
        "candidateDigest": commonkit_skills::digest_bytes(candidate).expect("candidate digest"),
        "policyDigest": manifest.policy_digest.clone(),
        "policyPassed": true,
        "evaluation": {
            "schemaVersion": 1,
            "baselineBasisPoints": 5000,
            "candidateBasisPoints": 7000,
            "heldOutBaselineBasisPoints": 5000,
            "heldOutCandidateBasisPoints": 7000,
            "requiredCases": {"held":"passed"},
            "costMicros": 1,
            "harness": suite.harness.clone(),
            "scorerDigest": digest('9')
        }
    });
    fs::write(
        &harness,
        format!("#!/bin/sh\nprintf '%s' '{}'\n", harness_report.to_string()),
    )
    .expect("harness");
    fs::set_permissions(&harness, fs::Permissions::from_mode(0o755)).expect("harness mode");
    let config = SkillOptProviderConfig {
        environment_root: root.join("tool"),
        provider_lock: provider_lock.clone(),
        backend: SkillOptBackend::Mock,
        model: None,
        tasks_file: tasks,
        harness_executable: harness,
        harness_corpus: corpus,
        environment: BTreeMap::new(),
        timeout_seconds: 5,
    };
    let optimizer = SkillOptSleepOptimizer::new(config).expect("provider");
    let output = optimizer
        .optimize(&manifest, &suite, original)
        .expect("output");
    assert_eq!(output.candidate, b"# Review\n\nImproved safely.\n");
    assert_eq!(output.evaluation.baseline_basis_points, 5_000);
    assert_eq!(output.evaluation.candidate_basis_points, 7_000);
    assert_eq!(original, b"# Review\n\nOriginal.\n");
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn provider_rejects_reviewed_task_files_without_tasks() {
    let root = std::env::temp_dir().join(format!(
        "commonkit-provider-empty-tasks-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let bin = root.join("tool/bin");
    fs::create_dir_all(&bin).expect("bin");
    for name in ["python", "skillopt-sleep"] {
        let path = bin.join(name);
        fs::write(&path, "#!/bin/sh\nprintf '0.2.0\\n'\n").expect("fixture executable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
    }
    let lock = ProviderLock {
        id: id("skillopt"),
        version: "0.2.0".into(),
        adapter_contract: id("skillopt-sleep-v1"),
        source: ProviderSource::Pypi,
        package_digest: digest('8'),
        capabilities: BTreeSet::from([id("reviewed-tasks"), id("staged-skill"), id("json-report")]),
        source_revision: None,
    };
    fs::write(
        root.join("tool/commonkit-provider-lock.json"),
        serde_json::to_vec(&ProviderCheck {
            provider: lock.id.clone(),
            version: lock.version.clone(),
            adapter_contract: lock.adapter_contract.clone(),
            source: lock.source,
            package_digest: lock.package_digest.clone(),
            capabilities: lock.capabilities.clone(),
            compatible: true,
        })
        .expect("marker"),
    )
    .expect("marker");
    let tasks = root.join("tasks.json");
    fs::write(
        &tasks,
        "{\"format\":\"skillopt_sleep.tasks.v1\",\"reviewed\":true,\"tasks\":[]}",
    )
    .expect("tasks");
    let result = SkillOptSleepOptimizer::new(SkillOptProviderConfig {
        environment_root: root.join("tool"),
        provider_lock: lock,
        backend: SkillOptBackend::Mock,
        model: None,
        tasks_file: tasks,
        harness_executable: root.join("tool/bin/skillopt-sleep"),
        harness_corpus: root.clone(),
        environment: BTreeMap::new(),
        timeout_seconds: 5,
    });
    assert!(result.is_err());
    fs::remove_dir_all(root).expect("cleanup");
}
