use commonkit_contracts::{ReceiptState, Sha256Digest, StableId};
use commonkit_reconcile::{ReceiptError, ReceiptJournal};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

#[test]
fn records_a_hash_chained_success_path() {
    let mut journal = ReceiptJournal::new(
        StableId::parse("run-01").expect("run"),
        digest('a'),
        StableId::parse("laptop").expect("target"),
        digest('b'),
        digest('c'),
        digest('d'),
    )
    .expect("journal");
    journal
        .transition(ReceiptState::Applying)
        .expect("applying");
    journal
        .transition(ReceiptState::Verifying)
        .expect("verifying");
    journal
        .transition(ReceiptState::Succeeded)
        .expect("succeeded");

    assert_eq!(journal.receipt().state, ReceiptState::Succeeded);
    assert_eq!(journal.receipt().transitions.len(), 4);
    journal.verify_chain().expect("valid chain");
}

#[test]
fn enforces_recovery_and_terminal_state_transitions() {
    let mut journal = ReceiptJournal::new(
        StableId::parse("run-02").expect("run"),
        digest('a'),
        StableId::parse("laptop").expect("target"),
        digest('b'),
        digest('c'),
        digest('d'),
    )
    .expect("journal");
    assert!(matches!(
        journal.transition(ReceiptState::Succeeded),
        Err(ReceiptError::IllegalTransition { .. })
    ));
    journal
        .transition(ReceiptState::Applying)
        .expect("applying");
    journal
        .transition(ReceiptState::RecoveryRequired)
        .expect("recovery");
    journal
        .transition(ReceiptState::RollingBack)
        .expect("rolling back");
    journal
        .transition(ReceiptState::RolledBack)
        .expect("rolled back");
    assert!(matches!(
        journal.transition(ReceiptState::Applying),
        Err(ReceiptError::IllegalTransition { .. })
    ));
}
