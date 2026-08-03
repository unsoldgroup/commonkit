use std::cell::RefCell;

use commonkit_adapters::{
    GitHubOnboardingAdapter, GitHubOnboardingError, GitHubRepositoryObservation,
    GitHubRepositoryRequest, GitHubTransport, RepositoryApplyDisposition,
};
use commonkit_contracts::StableId;
use commonkit_contracts::portable_context::RepositoryRole;

#[derive(Default)]
struct FakeGitHub {
    observed: RefCell<Option<GitHubRepositoryObservation>>,
    creates: RefCell<usize>,
}

impl GitHubTransport for FakeGitHub {
    fn inspect_repository(
        &self,
        _owner: &str,
        _name: &str,
    ) -> Result<Option<GitHubRepositoryObservation>, GitHubOnboardingError> {
        Ok(self.observed.borrow().clone())
    }

    fn create_private_repository(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<GitHubRepositoryObservation, GitHubOnboardingError> {
        *self.creates.borrow_mut() += 1;
        let observation = GitHubRepositoryObservation {
            account_node_id: "U_user".into(),
            repository_node_id: "R_personal".into(),
            owner: owner.into(),
            name: name.into(),
            private: true,
        };
        *self.observed.borrow_mut() = Some(observation.clone());
        Ok(observation)
    }
}

fn request() -> GitHubRepositoryRequest {
    GitHubRepositoryRequest {
        id: StableId::parse("personal-repo-1").unwrap(),
        role: RepositoryRole::PersonalContext,
        owner: "alice".into(),
        owner_node_id: "U_user".into(),
        name: "commonkit-context".into(),
    }
}

#[test]
fn planning_is_read_only_and_confirmed_apply_is_idempotent() {
    let transport = FakeGitHub::default();
    let adapter = GitHubOnboardingAdapter::new(&transport);

    let plan = adapter.inspect_and_plan(request()).unwrap();
    assert_eq!(*transport.creates.borrow(), 0);
    assert_eq!(plan.confirmation_digest, plan.digest().unwrap());

    let first = adapter.apply(&plan, &plan.confirmation_digest).unwrap();
    assert_eq!(first.disposition, RepositoryApplyDisposition::Created);
    assert_eq!(*transport.creates.borrow(), 1);

    let resumed = adapter.apply(&plan, &plan.confirmation_digest).unwrap();
    assert_eq!(resumed.disposition, RepositoryApplyDisposition::Connected);
    assert_eq!(*transport.creates.borrow(), 1);
}

#[test]
fn personal_repository_rejects_owner_identity_substitution() {
    let transport = FakeGitHub::default();
    let adapter = GitHubOnboardingAdapter::new(&transport);
    let plan = adapter.inspect_and_plan(request()).unwrap();
    *transport.observed.borrow_mut() = Some(GitHubRepositoryObservation {
        account_node_id: "U_attacker".into(),
        repository_node_id: "R_other".into(),
        owner: "alice".into(),
        name: "commonkit-context".into(),
        private: true,
    });

    assert_eq!(
        adapter
            .apply(&plan, &plan.confirmation_digest)
            .unwrap_err()
            .to_string(),
        "personal-context repository owner does not match the authenticated user"
    );
    assert_eq!(*transport.creates.borrow(), 0);
}
