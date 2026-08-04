use std::collections::BTreeMap;

use commonkit_personal_context::{ProfileReviewSchedule, SIX_MONTH_REVIEW_MS};

#[test]
fn reminders_use_injected_time_and_durable_global_or_section_dismissals() {
    let mut schedule = ProfileReviewSchedule {
        last_reviewed_at_unix_ms: 1_000,
        global_dismissed_until_unix_ms: None,
        section_dismissed_until_unix_ms: BTreeMap::new(),
    };
    let due = 1_000 + SIX_MONTH_REVIEW_MS;
    assert!(!schedule.is_due("communication", due - 1, false));
    assert!(schedule.is_due("communication", due, false));
    assert!(schedule.is_due("communication", 2_000, true));

    schedule
        .section_dismissed_until_unix_ms
        .insert("communication".into(), due + 10);
    assert!(!schedule.is_due("communication", due, true));
    assert!(schedule.is_due("identity", due, false));

    schedule.global_dismissed_until_unix_ms = Some(due + 20);
    assert!(!schedule.is_due("identity", due, true));
}
