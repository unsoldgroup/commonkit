use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_contracts::StableId;
use commonkit_skills::{ScheduleKind, SkillEngine, SkillSchedule};

#[test]
fn schedule_survives_restart_deduplicates_and_enforces_budget() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("commonkit-schedule-{}-{nonce}", std::process::id()));
    fs::create_dir_all(root.join("repository/.agents/skills/review")).expect("skills");
    fs::write(
        root.join("repository/.agents/skills/review/SKILL.md"),
        "# Review\n",
    )
    .expect("skill");
    let engine = SkillEngine::open(root.join("repository"), root.join("state")).expect("engine");
    engine
        .configure_schedule(SkillSchedule {
            kind: ScheduleKind::CandidateGeneration,
            enabled: true,
            interval_seconds: 86_400,
            maximum_cost_micros_per_period: 100,
        })
        .expect("schedule");
    assert!(
        engine
            .claim_scheduled_run(&StableId::parse("run-1").expect("id"), 60)
            .expect("first")
    );
    assert!(
        !engine
            .claim_scheduled_run(&StableId::parse("run-1").expect("id"), 60)
            .expect("duplicate")
    );
    assert!(
        !engine
            .claim_scheduled_run(&StableId::parse("run-2").expect("id"), 50)
            .expect("budget")
    );
    drop(engine);
    let reopened = SkillEngine::open(root.join("repository"), root.join("state")).expect("reopen");
    assert_eq!(
        reopened
            .schedule_status()
            .expect("status")
            .spent_cost_micros,
        60
    );
    reopened.disable_schedule().expect("disable");
    assert!(!reopened.schedule_status().expect("status").schedule.enabled);
    fs::remove_dir_all(root).expect("cleanup");
}
