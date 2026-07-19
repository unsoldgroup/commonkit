use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use commonkit_service::{
    DomainFailure, DriftChecker, OverallState, SyncDomain, SyncDomainDriftChecker,
};
use serde_json::Value;

struct ReadOnlyDomain {
    verifies: AtomicUsize,
    drifted: bool,
}

impl SyncDomain for ReadOnlyDomain {
    fn plan(&self, _: Value) -> Result<Value, DomainFailure> {
        panic!("scheduled drift checks must not plan or mutate")
    }

    fn verify(&self, _: Value) -> Result<Value, DomainFailure> {
        self.verifies.fetch_add(1, Ordering::SeqCst);
        if self.drifted {
            Err(DomainFailure::VerificationFailed)
        } else {
            Ok(serde_json::json!({"verified":true}))
        }
    }

    fn rollback(&self, _: Value) -> Result<Value, DomainFailure> {
        panic!("scheduled drift checks must not roll back")
    }
}

#[test]
fn production_drift_checker_uses_only_domain_verification() {
    let domain = Arc::new(ReadOnlyDomain {
        verifies: AtomicUsize::new(0),
        drifted: true,
    });
    let checker = SyncDomainDriftChecker::new(Some(domain.clone()));
    let result = checker.check();
    assert_eq!(result.state, OverallState::Drifted);
    assert_eq!(result.code.as_deref(), Some("managed_state_drifted"));
    assert_eq!(domain.verifies.load(Ordering::SeqCst), 1);
}

#[test]
fn unconfigured_production_drift_checker_fails_closed() {
    let result = SyncDomainDriftChecker::new(None).check();
    assert_eq!(result.state, OverallState::Degraded);
    assert_eq!(result.code.as_deref(), Some("sync_domain_unconfigured"));
}
