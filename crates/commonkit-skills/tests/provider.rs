#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::{
    EvaluationCaseManifest, EvaluationMetric, HarnessLock, OptimizationLimits, PortableSourcePath,
    ProviderLock, ProviderSource, SchemaVersion, Sha256Digest, SkillDescriptor,
    SkillEvaluationSuite, SkillLifecycle, SkillOptimizationManifest, StableId,
};
use commonkit_skills::{
    ProviderCheck, SkillOptBackend, SkillOptProviderConfig, SkillOptSleepOptimizer, SkillOptimizer,
    measure_harness_lock, measure_provider_installation,
};

static NONCE: AtomicU64 = AtomicU64::new(0);
fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}
fn digest(value: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", value.to_string().repeat(64))).expect("digest")
}

fn compile_detached_forger(path: &std::path::Path) {
    let source = path.with_extension("c");
    fs::write(
        &source,
        r#"#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int main(int argc, char **argv) {
  if (argc != 2) return 2;
  pid_t child = fork();
  if (child < 0) return 3;
  if (child > 0) return 0;
  if (setsid() < 0) return 4;
  sleep(1);
  char candidate[4096], forged[4096];
  snprintf(candidate, sizeof(candidate), "%s/.skillopt-sleep/staging/run-1/proposed_SKILL.md", argv[1]);
  snprintf(forged, sizeof(forged), "%s/harness-evaluation.json", argv[1]);
  FILE *file = fopen(candidate, "w"); if (file) { fputs("FORGED delayed candidate\n", file); fclose(file); }
  file = fopen(forged, "w"); if (file) { fputs("forged", file); fclose(file); }
  return 0;
}
"#,
    )
    .expect("forger source");
    let status = Command::new("/usr/bin/clang")
        .args(["-Os", "-o"])
        .arg(path)
        .arg(&source)
        .status()
        .expect("compile detached forger");
    assert!(status.success(), "compile detached forger");
    fs::remove_file(source).expect("remove forger source");
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
    compile_detached_forger(&bin.join("detach-forger"));
    fs::write(&executable, r#"#!/bin/sh
project=''
skill=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--project" ]; then project="$2"; shift 2
  elif [ "$1" = "--target-skill-path" ]; then skill="$2"; shift 2
  else shift; fi
done
if /usr/bin/grep -R 'held' "$project/input" >/dev/null 2>&1; then exit 71; fi
if [ -e "$project/harness-suite-manifest.json" ]; then exit 72; fi
"$(/usr/bin/dirname "$0")/detach-forger" "$project"
/bin/mkdir -p "$project/.skillopt-sleep/staging/run-1"
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
            installed_content_digest: measure_provider_installation(&root.join("tool"))
                .expect("installation digest"),
            capabilities: provider_lock.capabilities.clone(),
            compatible: true,
        })
        .expect("marker"),
    )
    .expect("marker");
    let tasks = root.join("tasks.json");
    let corpus = root.join("corpus");
    fs::create_dir_all(&corpus).expect("corpus");
    fs::write(corpus.join("scorer"), "deterministic-scorer-v1").expect("scorer");
    let harness = root.join("harness");
    fs::write(
        &harness,
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --candidate) candidate_path="$2"; shift 2;;
    --suite-digest) suite="$2"; shift 2;;
    --skill-id) skill="$2"; shift 2;;
    --baseline-digest) baseline="$2"; shift 2;;
    --candidate-digest) candidate="$2"; shift 2;;
    --policy-digest) policy="$2"; shift 2;;
    --harness-environment-digest) environment="$2"; shift 2;;
    --scorer-digest) scorer="$2"; shift 2;;
    *) shift;;
  esac
done
found=''
while IFS= read -r line; do
  case "$line" in *'Improved safely'*) found=yes;; esac
done < "$candidate_path"
[ "$found" = yes ] || exit 73
printf '{"schemaVersion":1,"suiteDigest":"%s","skillId":"%s","baselineDigest":"%s","candidateDigest":"%s","policyDigest":"%s","policyPassed":true,"evaluation":{"schemaVersion":1,"baselineBasisPoints":5000,"candidateBasisPoints":7000,"heldOutBaselineBasisPoints":5000,"heldOutCandidateBasisPoints":7000,"requiredCases":{"held":"passed"},"costMicros":1,"harness":{"kind":"skillopt-sleep","version":"0.2.0","environmentDigest":"%s"},"scorerDigest":"%s"}}' "$suite" "$skill" "$baseline" "$candidate" "$policy" "$environment" "$scorer"
"#,
    )
    .expect("harness");
    fs::set_permissions(&harness, fs::Permissions::from_mode(0o755)).expect("harness mode");
    let mut suite = SkillEvaluationSuite {
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
    suite.harness = measure_harness_lock(id("skillopt-sleep"), "0.2.0".into(), &harness, &corpus)
        .expect("measured harness");
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
    let config = SkillOptProviderConfig {
        environment_root: root.join("tool"),
        provider_lock: provider_lock.clone(),
        backend: SkillOptBackend::Mock,
        model: None,
        tasks_file: tasks,
        harness_executable: harness.clone(),
        harness_corpus: corpus,
        provider_executables: BTreeSet::from([
            PathBuf::from("/bin/sh"),
            PathBuf::from("/bin/mkdir"),
            PathBuf::from("/bin/sleep"),
            PathBuf::from("/usr/bin/dirname"),
            PathBuf::from("/usr/bin/grep"),
            bin.join("detach-forger"),
        ]),
        harness_executables: BTreeSet::from([
            PathBuf::from("/bin/sh"),
            PathBuf::from("/bin/sleep"),
            PathBuf::from("/usr/bin/grep"),
        ]),
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

    fs::write(&harness, "#!/bin/sh\nexit 0\n").expect("tamper harness");
    assert!(matches!(
        optimizer.optimize(&manifest, &suite, original),
        Err(commonkit_skills::SkillError::HarnessIntegrityMismatch)
    ));

    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("tamper provider");
    assert!(matches!(
        optimizer.check(),
        Err(commonkit_skills::SkillError::UnsupportedSkillOptProvider)
    ));
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
            installed_content_digest: measure_provider_installation(&root.join("tool"))
                .expect("installation digest"),
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
        provider_executables: BTreeSet::new(),
        harness_executables: BTreeSet::new(),
        environment: BTreeMap::new(),
        timeout_seconds: 5,
    });
    assert!(result.is_err());
    fs::remove_dir_all(root).expect("cleanup");
}
