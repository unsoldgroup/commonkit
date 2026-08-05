//! Read-only package and toolchain observation.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::process::Command;

use commonkit_contracts::{PackageDeclaration, PackageManager, SecurityPolicy, StableId};
use commonkit_core::enforce_package_source_policy;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageDriftState {
    Satisfied,
    Missing,
    VersionMismatch,
    UnobservableBecauseBackendUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageDriftEntry {
    pub id: StableId,
    pub manager: PackageManager,
    pub source: StableId,
    pub desired_version: String,
    pub observed_versions: Vec<String>,
    pub state: PackageDriftState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_violation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageBackendEvidence {
    pub manager: PackageManager,
    pub manager_version: Option<String>,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageDriftReport {
    pub platform: String,
    pub architecture: String,
    pub backends: Vec<PackageBackendEvidence>,
    pub packages: Vec<PackageDriftEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub trait PackageCommandRunner: Send + Sync {
    fn run(
        &self,
        executable: &str,
        args: &[&str],
    ) -> Result<PackageCommandOutput, PackageCommandError>;
}

#[derive(Debug, Default)]
pub struct ProcessPackageCommandRunner;

impl PackageCommandRunner for ProcessPackageCommandRunner {
    fn run(
        &self,
        executable: &str,
        args: &[&str],
    ) -> Result<PackageCommandOutput, PackageCommandError> {
        let output = Command::new(executable)
            .args(args)
            .output()
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    PackageCommandError::BackendUnavailable(executable.into())
                } else {
                    PackageCommandError::Execution(error.to_string())
                }
            })?;
        if !output.status.success() {
            return Err(PackageCommandError::QueryFailed {
                executable: executable.into(),
                reason: String::from_utf8_lossy(&output.stderr).trim().into(),
            });
        }
        Ok(PackageCommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PackageCommandError {
    #[error("package backend executable is unavailable: {0}")]
    BackendUnavailable(String),
    #[error("package observation command failed for {executable}: {reason}")]
    QueryFailed { executable: String, reason: String },
    #[error("package observation command could not execute: {0}")]
    Execution(String),
}

pub struct PackageObserver<R> {
    runner: R,
}

impl<R> PackageObserver<R>
where
    R: PackageCommandRunner,
{
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub fn observe(
        &self,
        declarations: &[PackageDeclaration],
        policy: &SecurityPolicy,
    ) -> PackageDriftReport {
        let managers = declarations
            .iter()
            .map(|declaration| declaration.manager)
            .collect::<BTreeSet<_>>();
        let mut observations = BTreeMap::new();
        let mut backends = Vec::with_capacity(managers.len());
        for manager in managers {
            match self.observe_backend(manager) {
                Ok((version, installed)) => {
                    backends.push(PackageBackendEvidence {
                        manager,
                        manager_version: Some(version),
                        available: true,
                        reason: None,
                    });
                    observations.insert(manager, Ok(installed));
                }
                Err(error) => {
                    let reason = error.to_string();
                    backends.push(PackageBackendEvidence {
                        manager,
                        manager_version: None,
                        available: false,
                        reason: Some(reason.clone()),
                    });
                    observations.insert(manager, Err(reason));
                }
            }
        }

        let packages = declarations
            .iter()
            .map(|declaration| {
                let policy_violation = enforce_package_source_policy(policy, declaration)
                    .err()
                    .map(|error| error.to_string());
                match observations.get(&declaration.manager) {
                    Some(Ok(installed)) => {
                        let observed_versions = installed
                            .get(declaration.id.as_str())
                            .cloned()
                            .unwrap_or_default();
                        let state = if observed_versions.is_empty() {
                            PackageDriftState::Missing
                        } else if observed_versions.contains(&declaration.version) {
                            PackageDriftState::Satisfied
                        } else {
                            PackageDriftState::VersionMismatch
                        };
                        PackageDriftEntry {
                            id: declaration.id.clone(),
                            manager: declaration.manager,
                            source: declaration.source.clone(),
                            desired_version: declaration.version.clone(),
                            observed_versions,
                            state,
                            reason: None,
                            policy_violation,
                        }
                    }
                    Some(Err(reason)) => PackageDriftEntry {
                        id: declaration.id.clone(),
                        manager: declaration.manager,
                        source: declaration.source.clone(),
                        desired_version: declaration.version.clone(),
                        observed_versions: Vec::new(),
                        state: PackageDriftState::UnobservableBecauseBackendUnavailable,
                        reason: Some(reason.clone()),
                        policy_violation,
                    },
                    None => unreachable!("every declared manager was observed"),
                }
            })
            .collect();
        PackageDriftReport {
            platform: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            backends,
            packages,
        }
    }

    fn observe_backend(
        &self,
        manager: PackageManager,
    ) -> Result<(String, BTreeMap<String, Vec<String>>), PackageCommandError> {
        let (executable, version_args, list_args) = commands(manager);
        let version = self.runner.run(executable, version_args)?.stdout;
        let list = self.runner.run(executable, list_args)?.stdout;
        Ok((
            first_nonempty_line(&version),
            parse_backend_output(manager, &list),
        ))
    }
}

fn commands(
    manager: PackageManager,
) -> (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
) {
    match manager {
        PackageManager::Homebrew => ("brew", &["--version"], &["list", "--versions"]),
        PackageManager::Apt => ("apt", &["--version"], &["list", "--installed"]),
        PackageManager::Fnm => ("fnm", &["--version"], &["list"]),
        PackageManager::Nvm => ("nvm", &["--version"], &["ls"]),
        PackageManager::Rustup => ("rustup", &["--version"], &["toolchain", "list"]),
    }
}

fn first_nonempty_line(output: &str) -> String {
    output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("unknown")
        .trim()
        .into()
}

/// Parses the real textual output of a fixed package observation command.
pub fn parse_backend_output(
    manager: PackageManager,
    output: &str,
) -> BTreeMap<String, Vec<String>> {
    match manager {
        PackageManager::Homebrew => parse_homebrew(output),
        PackageManager::Apt => parse_apt(output),
        PackageManager::Fnm | PackageManager::Nvm => parse_node_versions(output),
        PackageManager::Rustup => parse_rustup(output),
    }
}

fn parse_homebrew(output: &str) -> BTreeMap<String, Vec<String>> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.into(), fields.map(str::to_owned).collect()))
        })
        .collect()
}

fn parse_apt(output: &str) -> BTreeMap<String, Vec<String>> {
    output
        .lines()
        .filter_map(|line| {
            let (name_and_repo, rest) = line.split_once(' ')?;
            if !rest.contains("[installed") {
                return None;
            }
            let name = name_and_repo.split('/').next()?;
            let version = rest.split_whitespace().next()?;
            Some((name.into(), vec![version.into()]))
        })
        .collect()
}

fn parse_node_versions(output: &str) -> BTreeMap<String, Vec<String>> {
    let versions = output
        .lines()
        .filter_map(|line| {
            line.split_whitespace()
                .find(|field| {
                    field.strip_prefix('v').is_some_and(|version| {
                        version.chars().next().is_some_and(|c| c.is_ascii_digit())
                    })
                })
                .map(|field| {
                    field
                        .trim_matches(|character: char| {
                            !character.is_ascii_alphanumeric()
                                && character != '.'
                                && character != '-'
                                && character != '+'
                        })
                        .trim_start_matches('v')
                        .to_owned()
                })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    BTreeMap::from([("node".into(), versions)])
}

fn parse_rustup(output: &str) -> BTreeMap<String, Vec<String>> {
    let mut installed = BTreeMap::<String, Vec<String>>::new();
    for toolchain in output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        let id = if toolchain.starts_with("stable-") {
            "stable"
        } else if toolchain.starts_with("beta-") {
            "beta"
        } else if toolchain.starts_with("nightly-") {
            "nightly"
        } else {
            "rust"
        };
        installed
            .entry(id.into())
            .or_default()
            .push(toolchain.into());
    }
    installed
}
