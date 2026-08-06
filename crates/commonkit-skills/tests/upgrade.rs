#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

use commonkit_contracts::{ProviderLock, ProviderSource, Sha256Digest, StableId};
#[cfg(not(target_os = "macos"))]
use commonkit_skills::SkillError;
use commonkit_skills::SkillOptProviderManager;
#[cfg(target_os = "macos")]
use commonkit_skills::UpgradeApproval;

static NONCE: AtomicU64 = AtomicU64::new(0);
fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}

#[test]
fn upgrade_uses_disposable_hashed_environment_and_requires_supported_platform_isolation() {
    let root = std::env::temp_dir().join(format!(
        "commonkit-upgrade-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).expect("root");
    let uv = root.join("fake-uv");
    fs::write(&uv, r#"#!/bin/sh
if [ "$1" = "venv" ]; then
  envroot="$2"; mkdir -p "$envroot/bin"
  printf '#!/bin/sh\nprintf "0.2.0\\n"\n' > "$envroot/bin/python"
  chmod 755 "$envroot/bin/python"
  cat > "$envroot/bin/skillopt-sleep" <<'SCRIPT'
#!/bin/sh
project=''; skill=''; tasks=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--project" ]; then project="$2"; shift 2
  elif [ "$1" = "--target-skill-path" ]; then skill="$2"; shift 2
  elif [ "$1" = "--tasks-file" ]; then tasks="$2"; shift 2
  else shift; fi
done
/bin/mkdir -p "$project/.skillopt-sleep/staging/contract"
printf '# Contract fixture\n\nAnswer accurately. Always wrap the final answer in <answer>...</answer> tags.\n' > "$project/.skillopt-sleep/staging/contract/proposed_SKILL.md"
printf '{"live_skill_path":"%s","live_memory_path":"","has_skill":true,"has_memory":false,"accepted":true}' "$skill" > "$project/.skillopt-sleep/staging/contract/manifest.json"
printf '{"night":1,"accepted":true,"gate_action":"accept_new_best","no_edits_reason":"","baseline":0.0,"candidate":1.0,"n_tasks":2,"n_sessions":0,"n_accepted_edits":1,"n_rejected_edits":0,"edits":[{"target":"skill"}],"rejected_edits":[],"notes":[],"staging_dir":"%s/.skillopt-sleep/staging/contract","adopted":[],"tasks_file":"%s","tasks_reviewed":true}' "$project" "$tasks"
SCRIPT
  chmod 755 "$envroot/bin/skillopt-sleep"
elif [ "$1" = "pip" ] && [ "$2" = "compile" ]; then
  while [ "$#" -gt 0 ]; do if [ "$1" = "--output-file" ]; then out="$2"; break; fi; shift; done
  printf 'skillopt==0.2.0 --hash=sha256:818db802507c6f82553fd24c75aa70c953ab0a712647f60e68e4595052c4b150\n' > "$out"
elif [ "$1" = "pip" ] && [ "$2" = "sync" ]; then
  exit 0
else
  exit 9
fi
"#).expect("uv");
    fs::set_permissions(&uv, fs::Permissions::from_mode(0o755)).expect("mode");
    let fixtures = root.join("fixtures");
    fs::create_dir_all(&fixtures).expect("fixtures");
    fs::write(
        fixtures.join("SKILL.md"),
        "# Contract fixture\n\nAnswer accurately.\n",
    )
    .expect("skill");
    fs::write(
        fixtures.join("reviewed-tasks.json"),
        "{\"format\":\"skillopt_sleep.tasks.v1\",\"reviewed\":true,\"commonkit\":{\"skillId\":\"contract-fixture\",\"campaignId\":\"provider-contract\",\"suiteDigest\":\"sha256:441b280fee317839802de9768353550be4f655bdb0529cc99b279f8a67cd6d2e\",\"trainDigest\":\"sha256:116f54c41d0405dbb10e7b04ebc31e262a5b8c85c1233fcf36eaee344d91ae58\",\"validationDigest\":\"sha256:98c41dcd20b86b86830ec0794559835614458ceaae0f0ec77a3ed1cd3a1f7d55\"},\"tasks\":[{\"id\":\"wrap-train\"},{\"id\":\"wrap-validation\"}]}",
    )
    .expect("tasks");
    let lock = ProviderLock {
        id: id("skillopt"),
        version: "0.2.0".into(),
        adapter_contract: id("skillopt-sleep-v1"),
        source: ProviderSource::Pypi,
        package_digest: Sha256Digest::parse(
            "sha256:818db802507c6f82553fd24c75aa70c953ab0a712647f60e68e4595052c4b150",
        )
        .expect("digest"),
        capabilities: BTreeSet::from([id("reviewed-tasks"), id("staged-skill"), id("json-report")]),
        source_revision: None,
    };
    let corpus = root.join("held-out");
    fs::create_dir(&corpus).expect("corpus");
    fs::write(corpus.join("scorer"), b"scorer").expect("scorer");
    let harness = root.join("harness");
    fs::write(
        &harness,
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
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
printf '{"schemaVersion":1,"suiteDigest":"%s","skillId":"%s","baselineDigest":"%s","candidateDigest":"%s","policyDigest":"%s","policyPassed":true,"evaluation":{"schemaVersion":1,"baselineBasisPoints":0,"candidateBasisPoints":10000,"heldOutBaselineBasisPoints":10000,"heldOutCandidateBasisPoints":10000,"requiredCases":{"wrap-held-out":"passed"},"costMicros":1,"harness":{"kind":"skillopt-sleep","version":"0.2.0","environmentDigest":"%s"},"scorerDigest":"%s"}}' "$suite" "$skill" "$baseline" "$candidate" "$policy" "$environment" "$scorer"
"#,
    )
    .unwrap();
    fs::set_permissions(&harness, fs::Permissions::from_mode(0o755)).unwrap();
    let suite = commonkit_skills::measured_provider_fixture_suite(&harness, &corpus)
        .expect("measured fixture suite");
    fs::write(
        fixtures.join("reviewed-tasks.json"),
        serde_json::to_vec(&serde_json::json!({
            "format":"skillopt_sleep.tasks.v1",
            "reviewed":true,
            "commonkit":{
                "skillId":"contract-fixture",
                "campaignId":"provider-contract",
                "suiteDigest":suite.digest().expect("suite digest"),
                "trainDigest":suite.train.content_digest,
                "validationDigest":suite.validation.content_digest
            },
            "tasks":[{"id":"wrap-train"},{"id":"wrap-validation"}]
        }))
        .expect("tasks"),
    )
    .expect("tasks");
    let manager = SkillOptProviderManager::open(&uv, root.join("providers"))
        .expect("manager")
        .with_harness(&harness, &corpus)
        .expect("harness")
        .with_declared_executables(
            BTreeSet::from([std::path::PathBuf::from("/bin/mkdir")]),
            BTreeSet::new(),
        )
        .expect("declared executables");
    let plan = manager.plan_upgrade(lock.clone(), &fixtures).expect("plan");
    #[cfg(not(target_os = "macos"))]
    {
        let error = manager
            .execute_upgrade(&plan)
            .expect_err("upgrade evaluation must fail closed without OS isolation");
        assert!(matches!(error, SkillError::ProviderIsolationUnavailable));
        assert!(!root.join("active-provider.json").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[cfg(target_os = "macos")]
    let report = manager.execute_upgrade(&plan).expect("upgrade report");
    #[cfg(target_os = "macos")]
    {
        assert!(report.compatible);
        assert_eq!(report.baseline_basis_points, 0);
        assert_eq!(report.candidate_basis_points, 10_000);
        let active_lock = root.join("active-provider.json");
        assert!(!active_lock.exists());
        manager
            .activate_upgrade(
                &report,
                UpgradeApproval {
                    approver: id("al"),
                    approved_at_unix_ms: 1,
                    reason: "contract and behavior reviewed".into(),
                },
                &active_lock,
            )
            .expect("activate");
        let activated: ProviderLock =
            serde_json::from_slice(&fs::read(active_lock).expect("active lock"))
                .expect("lock json");
        assert_eq!(activated, lock);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
