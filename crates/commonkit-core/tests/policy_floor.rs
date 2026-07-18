use std::collections::{BTreeMap, BTreeSet};

use commonkit_core::{PolicyViolation, SecurityPolicy, StableId, enforce_policy_floor};

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("ID")
}

fn organization_policy() -> SecurityPolicy {
    SecurityPolicy {
        denied_paths: BTreeSet::from(["**/.env".into()]),
        required_controls: BTreeMap::from([(id("secret_scan"), true)]),
        allowlists: BTreeMap::from([(
            id("git_hosts"),
            BTreeSet::from(["github.com".into(), "git.internal".into()]),
        )]),
        minimums: BTreeMap::from([(id("backup_count"), 1)]),
        maximums: BTreeMap::from([(id("snapshot_age_hours"), 24)]),
    }
}

#[test]
fn accepts_policy_that_only_tightens_the_organization_floor() {
    let mut effective = organization_policy();
    effective.denied_paths.insert("**/*.pem".into());
    effective
        .allowlists
        .insert(id("git_hosts"), BTreeSet::from(["github.com".into()]));
    effective.minimums.insert(id("backup_count"), 3);
    effective.maximums.insert(id("snapshot_age_hours"), 12);

    enforce_policy_floor(&organization_policy(), &effective).expect("tightened policy");
}

#[test]
fn rejects_every_policy_weakening_class() {
    let baseline = organization_policy();

    let mut denial = baseline.clone();
    denial.denied_paths.clear();
    assert!(matches!(
        enforce_policy_floor(&baseline, &denial),
        Err(PolicyViolation::DenialRemoved { .. })
    ));

    let mut required = baseline.clone();
    required.required_controls.insert(id("secret_scan"), false);
    assert!(matches!(
        enforce_policy_floor(&baseline, &required),
        Err(PolicyViolation::RequiredControlWeakened { .. })
    ));

    let mut allowlist = baseline.clone();
    allowlist
        .allowlists
        .get_mut(&id("git_hosts"))
        .expect("allowlist")
        .insert("evil.example".into());
    assert!(matches!(
        enforce_policy_floor(&baseline, &allowlist),
        Err(PolicyViolation::AllowlistWidened { .. })
    ));

    let mut minimum = baseline.clone();
    minimum.minimums.insert(id("backup_count"), 0);
    assert!(matches!(
        enforce_policy_floor(&baseline, &minimum),
        Err(PolicyViolation::MinimumLowered { .. })
    ));

    let mut maximum = baseline.clone();
    maximum.maximums.insert(id("snapshot_age_hours"), 48);
    assert!(matches!(
        enforce_policy_floor(&baseline, &maximum),
        Err(PolicyViolation::MaximumRaised { .. })
    ));
}
