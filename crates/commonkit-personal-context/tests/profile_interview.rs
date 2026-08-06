use commonkit_contracts::portable_context::core_profile_schema;
use commonkit_personal_context::{InterviewError, ProfileInterview, SecretValue};

#[test]
fn interview_asks_one_question_at_a_time_and_requires_value_confirmation() {
    let mut interview = ProfileInterview::new(core_profile_schema());
    assert_eq!(interview.next_field(), Some("identity.display_name"));

    interview
        .answer_current(SecretValue::new("Alice Example"))
        .unwrap();
    assert_eq!(
        interview.advance().unwrap_err(),
        InterviewError::UnconfirmedValue
    );
    let confirmation = interview.pending_confirmation().unwrap();
    interview.confirm_pending(&confirmation).unwrap();
    interview.advance().unwrap();
    assert_eq!(interview.next_field(), Some("identity.pronouns"));

    let draft = interview.into_confirmed_draft().unwrap();
    assert_eq!(draft.values.len(), 1);
    assert_eq!(draft.values[0].field_id, "identity.display_name");
    assert_eq!(draft.values[0].value.expose(), b"Alice Example");
}

#[test]
fn cancellation_drops_the_unconfirmed_draft_without_a_portable_write() {
    let mut interview = ProfileInterview::new(core_profile_schema());
    interview
        .answer_current(SecretValue::new("do not persist"))
        .unwrap();
    let cancellation = interview.cancel();

    assert_eq!(cancellation.portable_writes, 0);
    assert_eq!(cancellation.persisted_transcript_bytes, 0);
}
