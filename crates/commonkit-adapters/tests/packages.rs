use std::collections::{BTreeMap, BTreeSet};

use commonkit_adapters::{
    PackageCommandError, PackageCommandOutput, PackageCommandRunner, PackageDriftState,
    PackageObserver, parse_backend_output,
};
use commonkit_contracts::{PackageDeclaration, PackageManager, SecurityPolicy, StableId};

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("stable ID")
}

#[test]
fn parses_fixed_backend_output_formats() {
    for (manager, fixture, expected_id, expected_version) in [
        (
            PackageManager::Homebrew,
            include_str!("fixtures/packages/brew.txt"),
            "ripgrep",
            "14.1.1",
        ),
        (
            PackageManager::Apt,
            include_str!("fixtures/packages/apt.txt"),
            "curl",
            "8.5.0-2ubuntu10.6",
        ),
        (
            PackageManager::Fnm,
            include_str!("fixtures/packages/fnm.txt"),
            "node",
            "24.4.1",
        ),
        (
            PackageManager::Nvm,
            include_str!("fixtures/packages/nvm.txt"),
            "node",
            "22.17.0",
        ),
        (
            PackageManager::Rustup,
            include_str!("fixtures/packages/rustup.txt"),
            "stable",
            "stable-aarch64-apple-darwin",
        ),
    ] {
        let parsed = parse_backend_output(manager, fixture);
        assert!(
            parsed
                .get(expected_id)
                .is_some_and(|versions| versions.iter().any(|version| version == expected_version))
        );
    }
}

#[derive(Default)]
struct FakeRunner {
    outputs: BTreeMap<(String, Vec<String>), Result<PackageCommandOutput, PackageCommandError>>,
}

impl PackageCommandRunner for FakeRunner {
    fn run(
        &self,
        executable: &str,
        args: &[&str],
    ) -> Result<PackageCommandOutput, PackageCommandError> {
        self.outputs
            .get(&(
                executable.into(),
                args.iter().map(|arg| (*arg).into()).collect(),
            ))
            .cloned()
            .unwrap_or_else(|| Err(PackageCommandError::BackendUnavailable(executable.into())))
    }
}

fn output(stdout: &str) -> Result<PackageCommandOutput, PackageCommandError> {
    Ok(PackageCommandOutput {
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn package(id_value: &str, version: &str) -> PackageDeclaration {
    PackageDeclaration {
        id: id(id_value),
        version: version.into(),
        manager: PackageManager::Homebrew,
        source: id("homebrew_core"),
    }
}

fn policy() -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            id("package_sources"),
            BTreeSet::from(["homebrew_core".into()]),
        )]),
        ..SecurityPolicy::default()
    }
}

#[test]
fn classifies_satisfied_missing_and_version_mismatch() {
    let runner = FakeRunner {
        outputs: BTreeMap::from([
            (
                ("brew".into(), vec!["--version".into()]),
                output("Homebrew 4.5.0\n"),
            ),
            (
                ("brew".into(), vec!["list".into(), "--versions".into()]),
                output("ripgrep 14.1.1\nfd 9.0.0\n"),
            ),
        ]),
    };
    let report = PackageObserver::new(runner).observe(
        &[
            package("ripgrep", "14.1.1"),
            package("jq", "1.7.1"),
            package("fd", "10.2.0"),
        ],
        &policy(),
    );

    assert_eq!(report.packages[0].state, PackageDriftState::Satisfied);
    assert_eq!(report.packages[1].state, PackageDriftState::Missing);
    assert_eq!(report.packages[2].state, PackageDriftState::VersionMismatch);
    assert_eq!(
        report.backends[0].manager_version.as_deref(),
        Some("Homebrew 4.5.0")
    );
    assert!(!report.platform.is_empty());
}

#[test]
fn absent_backend_fails_closed_for_every_declaration() {
    let report = PackageObserver::new(FakeRunner::default())
        .observe(&[package("ripgrep", "14.1.1")], &policy());

    assert_eq!(
        report.packages[0].state,
        PackageDriftState::UnobservableBecauseBackendUnavailable
    );
    assert!(
        report.packages[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("unavailable"))
    );
}

#[test]
fn disallowed_source_is_reported_as_a_policy_violation() {
    let runner = FakeRunner {
        outputs: BTreeMap::from([
            (
                ("brew".into(), vec!["--version".into()]),
                output("Homebrew 4.5.0\n"),
            ),
            (
                ("brew".into(), vec!["list".into(), "--versions".into()]),
                output("ripgrep 14.1.1\n"),
            ),
        ]),
    };
    let mut declaration = package("ripgrep", "14.1.1");
    declaration.source = id("untrusted_tap");

    let report = PackageObserver::new(runner).observe(&[declaration], &policy());

    assert!(
        report.packages[0]
            .policy_violation
            .as_deref()
            .is_some_and(|violation| violation.contains("not permitted"))
    );
}
