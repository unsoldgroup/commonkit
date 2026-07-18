use commonkit_core::{
    CaseSensitivity, OperatingSystem, PlatformFacts, RootAccess, TargetInventory, TargetRoot,
    TargetTransport,
};
use commonkit_core::{StableId, digest_domain_json};

fn inventory(machine: &str) -> TargetInventory {
    TargetInventory::new(
        StableId::parse("workstation").unwrap(),
        TargetTransport::Local {
            machine_id: machine.into(),
        },
        PlatformFacts {
            operating_system: OperatingSystem::Linux,
            architecture: "x86_64".into(),
            case_sensitivity: CaseSensitivity::Sensitive,
        },
        vec![TargetRoot::parse("home", "/home/al", RootAccess::ReadWrite).unwrap()],
    )
    .unwrap()
}

#[test]
fn inventory_round_trips_canonically_and_has_a_stable_identity() {
    let target = inventory("machine-123");
    let encoded = serde_json::to_string(&target).unwrap();
    let decoded: TargetInventory = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, target);
    assert_eq!(
        decoded.identity_digest().unwrap(),
        target.identity_digest().unwrap()
    );
    assert_eq!(
        target.identity_digest().unwrap(),
        digest_domain_json("commonkit.target-identity.v1", &target).unwrap()
    );
}

#[test]
fn identity_changes_when_machine_platform_or_roots_change() {
    let base = inventory("machine-123");
    let different_machine = inventory("machine-456");
    let different_case = TargetInventory::new(
        StableId::parse("workstation").unwrap(),
        TargetTransport::Local {
            machine_id: "machine-123".into(),
        },
        PlatformFacts {
            operating_system: OperatingSystem::Linux,
            architecture: "x86_64".into(),
            case_sensitivity: CaseSensitivity::Insensitive,
        },
        vec![TargetRoot::parse("home", "/home/al", RootAccess::ReadWrite).unwrap()],
    )
    .unwrap();
    let different_root = TargetInventory::new(
        StableId::parse("workstation").unwrap(),
        TargetTransport::Local {
            machine_id: "machine-123".into(),
        },
        PlatformFacts {
            operating_system: OperatingSystem::Linux,
            architecture: "x86_64".into(),
            case_sensitivity: CaseSensitivity::Sensitive,
        },
        vec![TargetRoot::parse("home", "/srv/al", RootAccess::ReadWrite).unwrap()],
    )
    .unwrap();

    assert_ne!(
        base.identity_digest().unwrap(),
        different_machine.identity_digest().unwrap()
    );
    assert_ne!(
        base.identity_digest().unwrap(),
        different_case.identity_digest().unwrap()
    );
    assert_ne!(
        base.identity_digest().unwrap(),
        different_root.identity_digest().unwrap()
    );
}

#[test]
fn inventory_rejects_unsafe_roots_and_duplicate_root_ids() {
    assert!(TargetRoot::parse("home", "home/al", RootAccess::ReadWrite).is_err());
    assert!(TargetRoot::parse("home", "/home/../root", RootAccess::ReadWrite).is_err());
    assert!(TargetRoot::parse("home", "C:\\Users\\Al", RootAccess::ReadWrite).is_err());

    let root = TargetRoot::parse("home", "/home/al", RootAccess::ReadWrite).unwrap();
    assert!(
        TargetInventory::new(
            StableId::parse("workstation").unwrap(),
            TargetTransport::Ssh {
                host: "dev.example.test".into(),
                port: 22,
                user: "al".into(),
                host_key_fingerprint: "SHA256:fixture".into(),
            },
            PlatformFacts {
                operating_system: OperatingSystem::Linux,
                architecture: "aarch64".into(),
                case_sensitivity: CaseSensitivity::Sensitive,
            },
            vec![root.clone(), root],
        )
        .is_err()
    );
}
