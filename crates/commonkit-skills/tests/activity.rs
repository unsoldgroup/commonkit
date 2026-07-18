use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_skills::SkillEngine;

#[test]
fn claude_session_end_log_is_only_a_repository_scoped_freshness_signal() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("commonkit-activity-{}-{nonce}", std::process::id()));
    let repository = root.join("repository");
    fs::create_dir_all(repository.join(".agents/skills/review")).expect("skill");
    fs::write(
        repository.join(".agents/skills/review/SKILL.md"),
        "# Review\n",
    )
    .expect("skill");
    let other = root.join("other");
    fs::create_dir_all(&other).expect("other");
    let log = root.join("session-end.log");
    fs::write(
        &log,
        format!(
            "2026-07-18T10:00:00Z\t{}\n2026-07-18T11:00:00Z\t{}\n2026-07-18T11:30:00Z\t{}\n2026-07-18T12:00:00Z\t{}\n",
            repository.display(),
            other.display(),
            root.join("removed-repository").display(),
            repository.display()
        ),
    )
    .expect("log");

    let status = SkillEngine::open(&repository, root.join("state"))
        .expect("engine")
        .claude_activity_since(&log, "2026-07-18T10:30:00Z")
        .expect("activity");
    assert!(status.has_new_activity);
    assert_eq!(status.marker_count, 1);
    assert_eq!(
        status.latest_timestamp.as_deref(),
        Some("2026-07-18T12:00:00Z")
    );
    assert!(
        !serde_json::to_string(&status)
            .expect("json")
            .contains(&other.display().to_string())
    );
    fs::remove_dir_all(root).expect("cleanup");
}
