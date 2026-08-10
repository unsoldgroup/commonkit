use commonkit_contracts::{PlanBindings, ReceiptState, Sha256Digest, StableId};
use commonkit_reconcile::{ReceiptError, ReceiptJournal};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn bindings() -> PlanBindings {
    PlanBindings {
        target_identity_digest: digest('3'),
        composed_loadout_digest: digest('4'),
        provider_inputs_digest: digest('5'),
        ownership_map_digest: digest('6'),
        artifact_set_digest: digest('7'),
    }
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
        bindings(),
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
        bindings(),
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

#[test]
fn records_one_way_forward_recovery_states() {
    let mut journal = ReceiptJournal::new(
        StableId::parse("run-forward").unwrap(),
        digest('a'),
        StableId::parse("laptop").unwrap(),
        digest('b'),
        digest('c'),
        digest('d'),
        bindings(),
    )
    .unwrap();
    journal.transition(ReceiptState::Applying).unwrap();
    journal
        .transition(ReceiptState::ForwardRecoveryRequired)
        .unwrap();
    journal.transition(ReceiptState::ConvergingForward).unwrap();
    journal.transition(ReceiptState::ForwardRecovered).unwrap();
    assert!(matches!(
        journal.transition(ReceiptState::RollingBack),
        Err(ReceiptError::IllegalTransition { .. })
    ));
    journal.verify_chain().unwrap();
}
