use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, Sha256Digest, StableId, digest_domain_json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    ArtifactEvidence, ManagerBindingV1, OfflineInstallRecipeV1, PackageFetchRequestV1,
    PackageObservationV1, PackageResolutionBackend, PackageResolutionDraftV1,
    PackageResolutionError, PackageResolutionProbeV1, PackageResolutionRequestV1, PackageTargetV1,
    ResolvedPackage, SourceBindingV1,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AptRepositoryConfigurationV1 {
    pub source_id: StableId,
    pub suite: String,
    pub components: BTreeSet<String>,
    pub signed_by: PathBuf,
    pub signing_authority: StableId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AptResolvedPackageV1 {
    pub name: String,
    pub version: String,
    pub architecture: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AptResolvedArchiveV1 {
    pub package_name: String,
    pub version: String,
    pub architecture: String,
    pub immutable_locator: String,
    pub upstream_checksum: Sha256Digest,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AptTransactionRisksV1 {
    pub held: BTreeSet<String>,
    pub removals: BTreeSet<String>,
    pub downgrades: BTreeSet<String>,
    pub replacements: BTreeSet<String>,
    pub unresolved_alternatives: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AptResolutionSnapshotV1 {
    pub target: PackageTargetV1,
    pub manager: ManagerBindingV1,
    pub before: PackageObservationV1,
    pub repository_revision: String,
    pub signed_metadata: Vec<ArtifactEvidence>,
    pub closure: Vec<AptResolvedPackageV1>,
    pub archives: Vec<AptResolvedArchiveV1>,
    pub risks: AptTransactionRisksV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AptResolutionSystemRequestV1 {
    pub declaration: PackageDeclaration,
    pub target: PackageTargetV1,
    pub manager: ManagerBindingV1,
    pub source_id: StableId,
    pub canonical_repository: String,
    pub registry_definition_digest: Sha256Digest,
    pub repository: AptRepositoryConfigurationV1,
}

pub trait AptResolutionCommandRunner {
    fn resolve(
        &mut self,
        request: &AptResolutionSystemRequestV1,
    ) -> Result<AptResolutionSnapshotV1, AptResolutionCommandError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AptCommandSpecV1 {
    pub executable: String,
    pub args: Vec<String>,
    pub network: bool,
}

#[derive(Debug, Default)]
pub struct ProcessAptResolutionCommandRunner;

impl ProcessAptResolutionCommandRunner {
    pub fn command_snapshot(
        request: &AptResolutionSystemRequestV1,
    ) -> Result<Vec<AptCommandSpecV1>, AptResolutionCommandError> {
        validate_system_request(request)?;
        Ok(apt_command_plan(request, "<private>"))
    }

    pub fn probe_manager_binding(
        target: &PackageTargetV1,
        canonical_repository: &str,
        repository: &AptRepositoryConfigurationV1,
    ) -> Result<ManagerBindingV1, AptResolutionCommandError> {
        validate_probe_request(target, canonical_repository, repository)?;
        if !cfg!(target_os = "linux") {
            return Err(AptResolutionCommandError::Unavailable(
                "the production APT resolver is supported only on Linux".into(),
            ));
        }
        validate_host_files(repository)?;
        let os_release_bytes = fs::read("/etc/os-release")
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        let os_release = String::from_utf8(os_release_bytes).map_err(|_| {
            AptResolutionCommandError::InvalidOutput("/etc/os-release is not UTF-8".into())
        })?;
        validate_host_target(target, &os_release)?;
        let apt_digest = content_digest(
            &fs::read("/usr/bin/apt-get")
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )?;
        let dpkg_digest = content_digest(
            &fs::read("/usr/bin/dpkg")
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )?;
        let key_digest = content_digest(
            &fs::read(&repository.signed_by)
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )?;
        let dpkg_config_digest = digest_dpkg_configuration(
            std::path::Path::new("/etc/dpkg/dpkg.cfg"),
            std::path::Path::new("/etc/dpkg/dpkg.cfg.d"),
        )?;
        let workspace = tempfile::Builder::new()
            .prefix("commonkit-apt-probe-")
            .tempdir()
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        let workspace_text = workspace.path().to_str().ok_or_else(|| {
            AptResolutionCommandError::Unavailable("private APT path is not UTF-8".into())
        })?;
        let commands = [
            AptCommandSpecV1 {
                executable: "/usr/bin/apt-get".into(),
                args: vec!["--version".into()],
                network: false,
            },
            AptCommandSpecV1 {
                executable: "/usr/bin/dpkg".into(),
                args: vec!["--version".into()],
                network: false,
            },
            AptCommandSpecV1 {
                executable: "/usr/bin/dpkg".into(),
                args: vec!["--print-architecture".into()],
                network: false,
            },
            AptCommandSpecV1 {
                executable: "/usr/bin/apt-config".into(),
                args: vec!["dump".into()],
                network: false,
            },
        ];
        let outputs = commands
            .iter()
            .map(|command| run_fixed_command(command, workspace_text, false))
            .collect::<Result<Vec<_>, _>>()?;
        let architecture = outputs[2].trim();
        if architecture != target.arch {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg architecture differs from the bound target".into(),
            ));
        }
        let source_line = apt_source_line_parts(target, canonical_repository, repository)?;
        manager_binding_from_evidence(
            &outputs[0],
            &outputs[1],
            &outputs[3],
            &source_line,
            architecture,
            &os_release,
            &apt_digest,
            &dpkg_digest,
            &key_digest,
            &dpkg_config_digest,
        )
    }
}

impl AptResolutionCommandRunner for ProcessAptResolutionCommandRunner {
    fn resolve(
        &mut self,
        request: &AptResolutionSystemRequestV1,
    ) -> Result<AptResolutionSnapshotV1, AptResolutionCommandError> {
        validate_system_request(request)?;
        if !cfg!(target_os = "linux") {
            return Err(AptResolutionCommandError::Unavailable(
                "the production APT resolver is supported only on Linux".into(),
            ));
        }
        resolve_apt_on_linux(request)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AptResolutionCommandError {
    #[error("required APT resolver capability is unavailable: {0}")]
    Unavailable(String),
    #[error("APT resolver command failed: {0}")]
    Failed(String),
    #[error("APT resolver returned invalid output: {0}")]
    InvalidOutput(String),
}

pub struct AptResolutionBackend<R> {
    repository: AptRepositoryConfigurationV1,
    runner: R,
    cached: Option<(Sha256Digest, AptResolutionSnapshotV1)>,
}

impl<R> AptResolutionBackend<R> {
    pub fn new(repository: AptRepositoryConfigurationV1, runner: R) -> Self {
        Self {
            repository,
            runner,
            cached: None,
        }
    }
}

impl<R> PackageResolutionBackend for AptResolutionBackend<R>
where
    R: AptResolutionCommandRunner,
{
    fn manager(&self) -> PackageManager {
        PackageManager::Apt
    }

    fn probe(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        let system_request = self.system_request(request)?;
        let binding = digest_domain_json("commonkit.apt-resolution-request.v1", &system_request)?;
        let mut snapshot = self.runner.resolve(&system_request).map_err(|error| {
            PackageResolutionError::AptBackend {
                reason: error.to_string(),
            }
        })?;
        validate_snapshot(&system_request, &mut snapshot)?;
        let probe = PackageResolutionProbeV1 {
            before: snapshot.before.clone(),
            repository_revision: Some(snapshot.repository_revision.clone()),
            signed_metadata: snapshot.signed_metadata.clone(),
        };
        self.cached = Some((binding, snapshot));
        Ok(probe)
    }

    fn resolve(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
        source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        let system_request = self.system_request(request)?;
        let binding = digest_domain_json("commonkit.apt-resolution-request.v1", &system_request)?;
        let (_, snapshot) = self
            .cached
            .take()
            .filter(|(cached_binding, _)| cached_binding == &binding)
            .ok_or(PackageResolutionError::ResolutionBindingMismatch)?;
        if source.source_id != system_request.source_id
            || source.canonical_repository != system_request.canonical_repository
            || source.registry_definition_digest != system_request.registry_definition_digest
            || source.repository_revision.as_deref() != Some(snapshot.repository_revision.as_str())
            || source.signed_metadata != snapshot.signed_metadata
        {
            return Err(PackageResolutionError::SourceBindingMismatch);
        }

        draft_from_snapshot(&system_request.declaration, source, snapshot)
    }
}

impl<R> AptResolutionBackend<R> {
    fn system_request(
        &self,
        request: &PackageResolutionRequestV1<'_>,
    ) -> Result<AptResolutionSystemRequestV1, PackageResolutionError> {
        let declaration = match request.desired {
            crate::PackageDesiredIntent::Package { declaration } => declaration,
        };
        let PackageSelector::AptBinary { architecture, .. } = declaration
            .selector
            .as_ref()
            .ok_or(PackageResolutionError::InvalidAptRequest)?
        else {
            return Err(PackageResolutionError::InvalidAptRequest);
        };
        let supported_distro = request
            .target
            .distro_id
            .as_deref()
            .is_some_and(|distro| matches!(distro, "debian" | "ubuntu"));
        if request.target.os != "linux"
            || !supported_distro
            || request.target.codename.as_deref() != Some(self.repository.suite.as_str())
            || architecture.as_deref() != Some(request.target.arch.as_str())
            || request.manager.manager != PackageManager::Apt
            || declaration.manager != PackageManager::Apt
            || declaration.source != self.repository.source_id
            || request.source_id != &self.repository.source_id
            || self.repository.components.is_empty()
            || !self.repository.signed_by.is_absolute()
            || !safe_apt_token(&request.target.arch)
            || !safe_apt_token(&self.repository.suite)
            || self
                .repository
                .components
                .iter()
                .any(|component| !safe_apt_token(component))
            || !valid_apt_repository(request.canonical_repository)
        {
            return Err(PackageResolutionError::InvalidAptRequest);
        }
        Ok(AptResolutionSystemRequestV1 {
            declaration: declaration.clone(),
            target: request.target.clone(),
            manager: request.manager.clone(),
            source_id: request.source_id.clone(),
            canonical_repository: request.canonical_repository.into(),
            registry_definition_digest: request.registry_definition_digest.clone(),
            repository: self.repository.clone(),
        })
    }
}

fn validate_snapshot(
    request: &AptResolutionSystemRequestV1,
    snapshot: &mut AptResolutionSnapshotV1,
) -> Result<(), PackageResolutionError> {
    if snapshot.target != request.target || snapshot.manager != request.manager {
        return Err(PackageResolutionError::AptAuthorityMismatch);
    }
    let immutable_revision = snapshot.repository_revision.len() == 64
        && snapshot
            .repository_revision
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase());
    if !immutable_revision
        || snapshot.signed_metadata.is_empty()
        || snapshot
            .signed_metadata
            .iter()
            .any(|evidence| evidence.authority != request.repository.signing_authority)
    {
        return Err(PackageResolutionError::UnauthenticatedAptMetadata);
    }
    if !snapshot.risks.held.is_empty()
        || !snapshot.risks.removals.is_empty()
        || !snapshot.risks.downgrades.is_empty()
        || !snapshot.risks.replacements.is_empty()
        || !snapshot.risks.unresolved_alternatives.is_empty()
    {
        return Err(PackageResolutionError::UnsafeAptTransaction);
    }

    snapshot.closure.sort_by(|left, right| {
        (&left.name, &left.architecture, &left.version).cmp(&(
            &right.name,
            &right.architecture,
            &right.version,
        ))
    });
    snapshot.archives.sort_by(|left, right| {
        (&left.package_name, &left.architecture, &left.version).cmp(&(
            &right.package_name,
            &right.architecture,
            &right.version,
        ))
    });
    let closure_keys = snapshot
        .closure
        .iter()
        .map(|package| (&package.name, &package.architecture, &package.version))
        .collect::<Vec<_>>();
    let archive_keys = snapshot
        .archives
        .iter()
        .map(|archive| {
            (
                &archive.package_name,
                &archive.architecture,
                &archive.version,
            )
        })
        .collect::<Vec<_>>();
    if closure_keys.is_empty()
        || closure_keys.windows(2).any(|pair| pair[0] == pair[1])
        || archive_keys.windows(2).any(|pair| pair[0] == pair[1])
        || closure_keys != archive_keys
        || snapshot.archives.iter().any(|archive| archive.size == 0)
    {
        return Err(PackageResolutionError::IncompleteAptClosure);
    }
    let PackageSelector::AptBinary { name, architecture } =
        request
            .declaration
            .selector
            .as_ref()
            .ok_or(PackageResolutionError::InvalidAptRequest)?
    else {
        return Err(PackageResolutionError::InvalidAptRequest);
    };
    let root_architecture = architecture.as_deref().unwrap_or(&request.target.arch);
    if !snapshot.closure.iter().any(|package| {
        package.name == *name
            && package.version == request.declaration.version
            && package.architecture == root_architecture
    }) {
        return Err(PackageResolutionError::MissingRootPackage);
    }
    Ok(())
}

fn draft_from_snapshot(
    root: &PackageDeclaration,
    source: &SourceBindingV1,
    snapshot: AptResolutionSnapshotV1,
) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
    let source_metadata_digest = source.metadata_digest()?;
    let root_key = apt_declaration_key(root)?;
    let mut closure = Vec::with_capacity(snapshot.closure.len());
    let mut artifacts = Vec::with_capacity(snapshot.archives.len());
    let mut roles = BTreeSet::new();
    for package in snapshot.closure {
        let key = (&package.name, &package.architecture, &package.version);
        let declaration = if key == (&root_key.0, &root_key.1, &root.version) {
            root.clone()
        } else {
            let id = stable_hashed_id(
                "apt-dep",
                &format!("{}:{}", package.name, package.architecture),
            )?;
            PackageDeclaration {
                id,
                version: package.version,
                manager: PackageManager::Apt,
                source: source.source_id.clone(),
                selector: Some(PackageSelector::AptBinary {
                    name: package.name,
                    architecture: Some(package.architecture),
                }),
            }
        };
        declaration.validate_for_resolution()?;
        closure.push(ResolvedPackage {
            declaration,
            source: source.clone(),
        });
    }
    for archive in snapshot.archives {
        let archive_key = format!(
            "{}:{}:{}",
            archive.package_name, archive.architecture, archive.version
        );
        let role = stable_hashed_id("apt-archive", &archive_key)?;
        let materialization_key = stable_hashed_id("apt-deb", &archive_key)?;
        roles.insert(role.clone());
        artifacts.push(PackageFetchRequestV1 {
            role,
            immutable_locator: archive.immutable_locator,
            upstream_checksum: archive.upstream_checksum,
            size: archive.size,
            materialization_key,
            source_metadata_digest: source_metadata_digest.clone(),
        });
    }
    Ok(PackageResolutionDraftV1 {
        closure,
        artifacts,
        recipe: OfflineInstallRecipeV1::AptArchives {
            artifact_roles: roles,
        },
    })
}

fn apt_declaration_key(
    declaration: &PackageDeclaration,
) -> Result<(String, String), PackageResolutionError> {
    let PackageSelector::AptBinary { name, architecture } = declaration
        .selector
        .as_ref()
        .ok_or(PackageResolutionError::InvalidAptRequest)?
    else {
        return Err(PackageResolutionError::InvalidAptRequest);
    };
    Ok((
        name.clone(),
        architecture.clone().unwrap_or_else(|| "all".into()),
    ))
}

fn stable_hashed_id(prefix: &str, value: &str) -> Result<StableId, PackageResolutionError> {
    let hash = format!("{:x}", Sha256::digest(value.as_bytes()));
    Ok(StableId::parse(format!("{prefix}-{}", &hash[..24]))?)
}

fn validate_system_request(
    request: &AptResolutionSystemRequestV1,
) -> Result<(), AptResolutionCommandError> {
    let PackageSelector::AptBinary { name, architecture } =
        request.declaration.selector.as_ref().ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("missing APT selector".into())
        })?
    else {
        return Err(AptResolutionCommandError::InvalidOutput(
            "selector is not an APT binary".into(),
        ));
    };
    if request.declaration.manager != PackageManager::Apt
        || request.manager.manager != PackageManager::Apt
        || request.target.os != "linux"
        || !request
            .target
            .distro_id
            .as_deref()
            .is_some_and(|distro| matches!(distro, "debian" | "ubuntu"))
        || request.target.codename.as_deref() != Some(request.repository.suite.as_str())
        || architecture.as_deref() != Some(request.target.arch.as_str())
        || request.source_id != request.repository.source_id
        || request.declaration.source != request.source_id
        || name.is_empty()
        || !safe_apt_token(&request.target.arch)
        || request.repository.components.is_empty()
        || !safe_apt_token(&request.repository.suite)
        || request
            .repository
            .components
            .iter()
            .any(|component| !safe_apt_token(component))
        || !request.repository.signed_by.is_absolute()
        || !valid_apt_repository(&request.canonical_repository)
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT request authority is incomplete or inconsistent".into(),
        ));
    }
    Ok(())
}

fn validate_probe_request(
    target: &PackageTargetV1,
    canonical_repository: &str,
    repository: &AptRepositoryConfigurationV1,
) -> Result<(), AptResolutionCommandError> {
    if target.os != "linux"
        || !target
            .distro_id
            .as_deref()
            .is_some_and(|distro| matches!(distro, "debian" | "ubuntu"))
        || target.codename.as_deref() != Some(repository.suite.as_str())
        || !safe_apt_token(&target.arch)
        || repository.components.is_empty()
        || !safe_apt_token(&repository.suite)
        || repository
            .components
            .iter()
            .any(|component| !safe_apt_token(component))
        || !repository.signed_by.is_absolute()
        || !valid_apt_repository(canonical_repository)
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT manager probe authority is incomplete or inconsistent".into(),
        ));
    }
    Ok(())
}

fn apt_command_plan(
    request: &AptResolutionSystemRequestV1,
    workspace: &str,
) -> Vec<AptCommandSpecV1> {
    let PackageSelector::AptBinary { name, architecture } = request
        .declaration
        .selector
        .as_ref()
        .expect("validated APT selector")
    else {
        unreachable!("validated APT selector")
    };
    let architecture = architecture.as_deref().unwrap_or(&request.target.arch);
    let package = format!("{name}:{architecture}");
    let exact = format!("{package}={}", request.declaration.version);
    let apt_options = vec![
        "-o".into(),
        format!("Dir::Etc::sourcelist={workspace}/sources.list"),
        "-o".into(),
        "Dir::Etc::sourceparts=-".into(),
        "-o".into(),
        format!("Dir::State::lists={workspace}/lists"),
        "-o".into(),
        format!("Dir::Cache::archives={workspace}/archives"),
        "-o".into(),
        "APT::Get::List-Cleanup=0".into(),
        "-o".into(),
        "Acquire::AllowInsecureRepositories=false".into(),
        "-o".into(),
        "Acquire::AllowWeakRepositories=false".into(),
        "-o".into(),
        "Acquire::AllowDowngradeToInsecureRepositories=false".into(),
        "-o".into(),
        "APT::Get::AllowUnauthenticated=false".into(),
        "-o".into(),
        "Acquire::Check-Valid-Until=true".into(),
        "-o".into(),
        "Acquire::Check-Date=true".into(),
        "-o".into(),
        "Acquire::By-Hash=force".into(),
        "-o".into(),
        "Acquire::http::AllowRedirect=false".into(),
        "-o".into(),
        "Acquire::https::AllowRedirect=false".into(),
        "-o".into(),
        "Acquire::http::Proxy=DIRECT".into(),
        "-o".into(),
        "Acquire::https::Proxy=DIRECT".into(),
        "-o".into(),
        "Acquire::Languages=none".into(),
    ];
    let apt = |mut args: Vec<String>| {
        let mut all = apt_options.clone();
        all.append(&mut args);
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-get".into(),
            args: all,
            network: false,
        }
    };
    vec![
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-get".into(),
            args: vec!["--version".into()],
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/dpkg".into(),
            args: vec!["--version".into()],
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/dpkg".into(),
            args: vec!["--print-architecture".into()],
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-config".into(),
            args: vec!["dump".into()],
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/dpkg-query".into(),
            args: vec![
                "-W".into(),
                "-f=${db:Status-Abbrev}\\t${Version}\\n".into(),
                package,
            ],
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-mark".into(),
            args: vec!["showhold".into()],
            network: false,
        },
        {
            let mut command = apt(vec!["update".into()]);
            command.network = true;
            command
        },
        apt(vec![
            "--simulate".into(),
            "--no-remove".into(),
            "--no-install-recommends".into(),
            "install".into(),
            exact.clone(),
        ]),
        apt(vec![
            "-o".into(),
            format!("Dir::State::status={workspace}/status"),
            "--print-uris".into(),
            "--download-only".into(),
            "--no-remove".into(),
            "--no-install-recommends".into(),
            "--reinstall".into(),
            "install".into(),
            exact,
        ]),
    ]
}

fn resolve_apt_on_linux(
    request: &AptResolutionSystemRequestV1,
) -> Result<AptResolutionSnapshotV1, AptResolutionCommandError> {
    validate_host_files(&request.repository)?;
    let os_release_bytes = fs::read("/etc/os-release")
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let os_release = String::from_utf8(os_release_bytes.clone()).map_err(|_| {
        AptResolutionCommandError::InvalidOutput("/etc/os-release is not UTF-8".into())
    })?;
    validate_host_target(&request.target, &os_release)?;

    let apt_bytes = fs::read("/usr/bin/apt-get")
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let dpkg_bytes = fs::read("/usr/bin/dpkg")
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let key_bytes = fs::read(&request.repository.signed_by)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let apt_digest = content_digest(&apt_bytes)?;
    let dpkg_digest = content_digest(&dpkg_bytes)?;
    let key_digest = content_digest(&key_bytes)?;
    let dpkg_config_digest = digest_dpkg_configuration(
        std::path::Path::new("/etc/dpkg/dpkg.cfg"),
        std::path::Path::new("/etc/dpkg/dpkg.cfg.d"),
    )?;

    let workspace = tempfile::Builder::new()
        .prefix("commonkit-apt-resolution-")
        .tempdir()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let workspace_path = workspace.path();
    for directory in [
        workspace_path.join("lists"),
        workspace_path.join("lists/partial"),
        workspace_path.join("archives"),
        workspace_path.join("archives/partial"),
    ] {
        fs::create_dir_all(directory)
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    }
    let source_line = apt_source_line_parts(
        &request.target,
        &request.canonical_repository,
        &request.repository,
    )?;
    fs::write(workspace_path.join("sources.list"), &source_line)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    fs::write(workspace_path.join("status"), [])
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;

    let workspace_text = workspace_path.to_str().ok_or_else(|| {
        AptResolutionCommandError::Unavailable("private APT path is not UTF-8".into())
    })?;
    let commands = apt_command_plan(request, workspace_text);
    let mut outputs = vec![String::new(); commands.len()];
    for (index, command) in commands.iter().enumerate().take(6) {
        outputs[index] = run_fixed_command(command, workspace_text, index == 4)?;
    }

    let architecture = outputs[2].trim();
    if architecture != request.target.arch {
        return Err(AptResolutionCommandError::InvalidOutput(
            "dpkg architecture differs from the bound target".into(),
        ));
    }
    let manager = manager_binding_from_evidence(
        &outputs[0],
        &outputs[1],
        &outputs[3],
        &source_line,
        architecture,
        &os_release,
        &apt_digest,
        &dpkg_digest,
        &key_digest,
        &dpkg_config_digest,
    )?;
    if manager != request.manager {
        return Err(AptResolutionCommandError::InvalidOutput(
            "local APT manager authority differs from the controller binding".into(),
        ));
    }
    for (index, command) in commands.iter().enumerate().skip(6) {
        outputs[index] = run_fixed_command(command, workspace_text, false)?;
    }

    let metadata_bytes = load_single_inrelease(&workspace_path.join("lists"))?;
    let metadata_digest = content_digest(&metadata_bytes)?;
    let repository_revision = metadata_digest
        .as_str()
        .strip_prefix("sha256:")
        .expect("validated digest prefix")
        .to_owned();
    let signed_metadata = vec![ArtifactEvidence {
        authority: request.repository.signing_authority.clone(),
        metadata_digest,
        signature_digest: key_digest,
    }];
    let archives = parse_print_uris(&outputs[8])?;
    let mut risks = parse_simulation_risks(&outputs[7], &outputs[5]);
    let closure_names = archives
        .iter()
        .map(|archive| archive.package_name.as_str())
        .collect::<BTreeSet<_>>();
    risks
        .held
        .retain(|package| closure_names.contains(package.as_str()));
    risks.downgrades = find_downgrades(&outputs[7], workspace_text)?;
    let closure = archives
        .iter()
        .map(|archive| AptResolvedPackageV1 {
            name: archive.package_name.clone(),
            version: archive.version.clone(),
            architecture: archive.architecture.clone(),
        })
        .collect();
    let before = PackageObservationV1 {
        installed_versions: parse_installed_versions(&outputs[4]),
    };

    if content_digest(
        &fs::read("/usr/bin/apt-get")
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
    )? != apt_digest
        || content_digest(
            &fs::read("/usr/bin/dpkg")
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )? != dpkg_digest
        || content_digest(
            &fs::read(&request.repository.signed_by)
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )? != signed_metadata[0].signature_digest
        || digest_dpkg_configuration(
            std::path::Path::new("/etc/dpkg/dpkg.cfg"),
            std::path::Path::new("/etc/dpkg/dpkg.cfg.d"),
        )? != dpkg_config_digest
    {
        return Err(AptResolutionCommandError::Failed(
            "APT executable or signing key changed during resolution".into(),
        ));
    }

    Ok(AptResolutionSnapshotV1 {
        target: request.target.clone(),
        manager,
        before,
        repository_revision,
        signed_metadata,
        closure,
        archives,
        risks,
    })
}

fn validate_host_files(
    repository: &AptRepositoryConfigurationV1,
) -> Result<(), AptResolutionCommandError> {
    for path in [
        PathBuf::from("/usr/bin/apt-get"),
        PathBuf::from("/usr/bin/apt-config"),
        PathBuf::from("/usr/bin/dpkg"),
        PathBuf::from("/usr/bin/dpkg-query"),
        PathBuf::from("/usr/bin/apt-mark"),
        repository.signed_by.clone(),
    ] {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AptResolutionCommandError::Unavailable(format!(
                "required path is not a no-follow regular file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_host_target(
    target: &PackageTargetV1,
    os_release: &str,
) -> Result<(), AptResolutionCommandError> {
    let field = |name: &str| {
        os_release.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key == name).then(|| value.trim().trim_matches('"'))
        })
    };
    if field("ID") != target.distro_id.as_deref()
        || field("VERSION_ID") != target.distro_version.as_deref()
        || field("VERSION_CODENAME") != target.codename.as_deref()
        || field("VERSION_ID") != Some(target.os_version.as_str())
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "host distribution differs from the bound APT target".into(),
        ));
    }
    Ok(())
}

fn apt_source_line_parts(
    target: &PackageTargetV1,
    canonical_repository: &str,
    repository: &AptRepositoryConfigurationV1,
) -> Result<String, AptResolutionCommandError> {
    let signed_by = repository.signed_by.to_str().ok_or_else(|| {
        AptResolutionCommandError::InvalidOutput("Signed-By path is not UTF-8".into())
    })?;
    if !safe_apt_token(&target.arch)
        || !safe_apt_token(&repository.suite)
        || repository
            .components
            .iter()
            .any(|component| !safe_apt_token(component))
        || signed_by.chars().any(char::is_whitespace)
        || signed_by.contains([']', '[', '\n', '\r'])
        || !valid_apt_repository(canonical_repository)
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT source configuration contains an unsafe token".into(),
        ));
    }
    Ok(format!(
        "deb [arch={} signed-by={signed_by}] {} {} {}\n",
        target.arch,
        canonical_repository,
        repository.suite,
        repository
            .components
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

fn safe_apt_token(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '+' | '-' | '_')
        })
}

fn valid_apt_repository(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && !url.cannot_be_a_base()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn run_fixed_command(
    command: &AptCommandSpecV1,
    workspace: &str,
    allow_not_installed: bool,
) -> Result<String, AptResolutionCommandError> {
    const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
    let output = Command::new(&command.executable)
        .args(&command.args)
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("HOME", workspace)
        .env("TMPDIR", workspace)
        .output()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let accepted = output.status.success()
        || (allow_not_installed && output.status.code().is_some_and(|code| code == 1));
    if !accepted {
        return Err(AptResolutionCommandError::Failed(
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1_024)
                .collect(),
        ));
    }
    if output.stdout.len() > MAX_OUTPUT_BYTES || output.stderr.len() > MAX_OUTPUT_BYTES {
        return Err(AptResolutionCommandError::Failed(
            "APT command output exceeded the fixed bound".into(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| AptResolutionCommandError::InvalidOutput("APT output is not UTF-8".into()))
}

fn parse_tool_version(output: &str, preceding: &str) -> Result<String, AptResolutionCommandError> {
    let fields = output
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>();
    let version = fields
        .windows(2)
        .find_map(|pair| (pair[0].trim_matches('\'') == preceding).then_some(pair[1]))
        .or_else(|| {
            fields.iter().copied().find(|field| {
                field
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_digit())
            })
        })
        .map(|value| {
            value.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '.' | '+' | '-' | '~')
            })
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("tool version is unavailable".into())
        })?;
    Ok(version.into())
}

#[allow(clippy::too_many_arguments)]
fn manager_binding_from_evidence(
    apt_version_output: &str,
    dpkg_version_output: &str,
    apt_config_output: &str,
    source_line: &str,
    architecture: &str,
    os_release: &str,
    apt_digest: &Sha256Digest,
    dpkg_digest: &Sha256Digest,
    key_digest: &Sha256Digest,
    dpkg_config_digest: &Sha256Digest,
) -> Result<ManagerBindingV1, AptResolutionCommandError> {
    let apt_version = parse_tool_version(apt_version_output, "apt")?;
    let dpkg_version = parse_tool_version(dpkg_version_output, "version")?;
    let executable_digest = digest_domain_json(
        "commonkit.apt-executable-binding.v1",
        &(apt_digest, dpkg_digest),
    )
    .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))?;
    let config_digest = digest_domain_json(
        "commonkit.apt-config-binding.v1",
        &(
            apt_config_output,
            source_line,
            key_digest,
            dpkg_config_digest,
            architecture,
            os_release,
        ),
    )
    .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))?;
    Ok(ManagerBindingV1 {
        manager: PackageManager::Apt,
        version: format!("apt:{apt_version};dpkg:{dpkg_version}"),
        executable_digest,
        config_digest,
    })
}

fn digest_dpkg_configuration(
    base: &std::path::Path,
    fragments: &std::path::Path,
) -> Result<Sha256Digest, AptResolutionCommandError> {
    let base_metadata = fs::symlink_metadata(base)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let fragments_metadata = fs::symlink_metadata(fragments)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if base_metadata.file_type().is_symlink()
        || !base_metadata.is_file()
        || fragments_metadata.file_type().is_symlink()
        || !fragments_metadata.is_dir()
    {
        return Err(AptResolutionCommandError::Unavailable(
            "dpkg configuration is not a no-follow file/directory set".into(),
        ));
    }
    let mut entries = vec![(
        "dpkg.cfg".to_owned(),
        content_digest(
            &fs::read(base)
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )?,
    )];
    let mut paths = fs::read_dir(fragments)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    paths.sort_by_key(|entry| entry.file_name());
    for entry in paths {
        let metadata = entry
            .file_type()
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        if metadata.is_symlink() || !metadata.is_file() {
            return Err(AptResolutionCommandError::Unavailable(
                "dpkg configuration fragment is not a no-follow regular file".into(),
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            AptResolutionCommandError::Unavailable(
                "dpkg configuration fragment name is not UTF-8".into(),
            )
        })?;
        let digest = content_digest(
            &fs::read(entry.path())
                .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?,
        )?;
        entries.push((name, digest));
    }
    digest_domain_json("commonkit.dpkg-config.v1", &entries)
        .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))
}

fn load_single_inrelease(lists: &std::path::Path) -> Result<Vec<u8>, AptResolutionCommandError> {
    let mut matches = fs::read_dir(lists)
        .map_err(|error| AptResolutionCommandError::Failed(error.to_string()))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with("_InRelease"))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    matches.sort();
    if matches.len() != 1 {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT did not produce exactly one authenticated InRelease snapshot".into(),
        ));
    }
    fs::read(&matches[0]).map_err(|error| AptResolutionCommandError::Failed(error.to_string()))
}

fn parse_installed_versions(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            let (status, version) = line.split_once('\t')?;
            status
                .trim()
                .starts_with("ii")
                .then(|| version.trim().into())
        })
        .collect()
}

fn find_downgrades(
    simulation: &str,
    workspace: &str,
) -> Result<BTreeSet<String>, AptResolutionCommandError> {
    let mut downgrades = BTreeSet::new();
    for line in simulation.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("Inst ") else {
            continue;
        };
        let Some(package) = rest.split_whitespace().next() else {
            continue;
        };
        let Some(old_start) = rest.find('[') else {
            continue;
        };
        let Some(old_end) = rest[old_start + 1..].find(']') else {
            return Err(AptResolutionCommandError::InvalidOutput(
                "malformed installed APT version".into(),
            ));
        };
        let old_version = &rest[old_start + 1..old_start + 1 + old_end];
        let after_old = &rest[old_start + 1 + old_end + 1..];
        let new_version = after_old
            .split_once('(')
            .and_then(|(_, value)| value.split_whitespace().next())
            .ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput("malformed candidate APT version".into())
            })?;
        let spec = AptCommandSpecV1 {
            executable: "/usr/bin/dpkg".into(),
            args: vec![
                "--compare-versions".into(),
                new_version.into(),
                "lt".into(),
                old_version.into(),
            ],
            network: false,
        };
        let status = Command::new(&spec.executable)
            .args(&spec.args)
            .env_clear()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("HOME", workspace)
            .env("TMPDIR", workspace)
            .status()
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        match status.code() {
            Some(0) => {
                downgrades.insert(package.into());
            }
            Some(1) => {}
            _ => {
                return Err(AptResolutionCommandError::Failed(
                    "dpkg version comparison failed".into(),
                ));
            }
        }
    }
    Ok(downgrades)
}

fn content_digest(bytes: &[u8]) -> Result<Sha256Digest, AptResolutionCommandError> {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))
}

fn parse_print_uris(output: &str) -> Result<Vec<AptResolvedArchiveV1>, AptResolutionCommandError> {
    let mut archives = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !line.starts_with('\'') {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err(AptResolutionCommandError::InvalidOutput(
                "malformed APT print-uris record".into(),
            ));
        }
        let locator = fields[0]
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput(
                    "APT archive URI is not single-quoted".into(),
                )
            })?;
        if !locator.starts_with("https://") {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT archive URI is not HTTPS".into(),
            ));
        }
        let filename = fields[1]
            .trim_matches('\'')
            .strip_suffix(".deb")
            .ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput("APT archive filename is not a deb".into())
            })?;
        let mut name_parts = filename.rsplitn(3, '_');
        let architecture = name_parts.next().unwrap_or_default();
        let version = name_parts.next().unwrap_or_default();
        let package_name = name_parts.next().unwrap_or_default();
        if package_name.is_empty()
            || version.is_empty()
            || architecture.is_empty()
            || !package_name.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '+' | '-' | '.')
            })
            || !architecture
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT archive identity is invalid".into(),
            ));
        }
        let size = fields[2].parse::<u64>().map_err(|_| {
            AptResolutionCommandError::InvalidOutput("APT archive size is invalid".into())
        })?;
        let checksum = fields[3].strip_prefix("SHA256:").ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("APT archive lacks a SHA-256 checksum".into())
        })?;
        let upstream_checksum = Sha256Digest::parse(format!("sha256:{checksum}"))
            .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))?;
        archives.push(AptResolvedArchiveV1 {
            package_name: package_name.into(),
            version: version.into(),
            architecture: architecture.into(),
            immutable_locator: locator.into(),
            upstream_checksum,
            size,
        });
    }
    if archives.is_empty() {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT returned no exact archive records".into(),
        ));
    }
    archives.sort_by(|left, right| {
        (&left.package_name, &left.architecture, &left.version).cmp(&(
            &right.package_name,
            &right.architecture,
            &right.version,
        ))
    });
    if archives.windows(2).any(|pair| {
        (
            &pair[0].package_name,
            &pair[0].architecture,
            &pair[0].version,
        ) == (
            &pair[1].package_name,
            &pair[1].architecture,
            &pair[1].version,
        )
    }) {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT returned duplicate archive records".into(),
        ));
    }
    Ok(archives)
}

fn parse_simulation_risks(simulation: &str, held: &str) -> AptTransactionRisksV1 {
    let mut risks = AptTransactionRisksV1 {
        held: held
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        ..AptTransactionRisksV1::default()
    };
    for line in simulation.lines().map(str::trim) {
        if let Some(package) = line
            .strip_prefix("Remv ")
            .and_then(|rest| rest.split_whitespace().next())
        {
            risks.removals.insert(package.into());
        }
        if let Some(package) = line.strip_prefix("CommonKit-Replacement: ") {
            risks.replacements.insert(package.into());
        }
        if let Some(package) = line.strip_prefix("CommonKit-Unresolved-Alternative: ") {
            risks.unresolved_alternatives.insert(package.into());
        }
    }
    risks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sha256_print_uris_fixture_into_exact_archives() {
        let archives =
            parse_print_uris(include_str!("../tests/fixtures/apt/print-uris.txt")).unwrap();

        assert_eq!(archives.len(), 2);
        assert_eq!(archives[0].package_name, "curl");
        assert_eq!(archives[0].version, "8.5.0-2ubuntu10.6");
        assert_eq!(archives[0].architecture, "amd64");
        assert_eq!(archives[1].package_name, "libc6");
    }

    #[test]
    fn rejects_weak_archive_checksum_metadata() {
        let error = parse_print_uris(include_str!("../tests/fixtures/apt/print-uris-weak.txt"))
            .unwrap_err();

        assert!(matches!(error, AptResolutionCommandError::InvalidOutput(_)));
    }

    #[test]
    fn simulation_parser_marks_removal_and_replacement_outcomes_unsafe() {
        let risks = parse_simulation_risks(
            include_str!("../tests/fixtures/apt/simulate-malicious.txt"),
            "held-package\n",
        );

        assert_eq!(risks.held, BTreeSet::from(["held-package".into()]));
        assert_eq!(risks.removals, BTreeSet::from(["old-lib".into()]));
        assert_eq!(
            risks.replacements,
            BTreeSet::from(["replacement-lib".into()])
        );
        assert_eq!(
            risks.unresolved_alternatives,
            BTreeSet::from(["virtual-mailer".into()])
        );
    }

    #[test]
    fn dpkg_configuration_digest_is_ordered_and_rejects_symlinked_fragments() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("dpkg.cfg");
        let fragments = root.path().join("dpkg.cfg.d");
        fs::create_dir(&fragments).unwrap();
        fs::write(&base, "force-confold\n").unwrap();
        fs::write(fragments.join("z-last"), "path-exclude=/tmp/*\n").unwrap();
        fs::write(fragments.join("a-first"), "no-debsig\n").unwrap();

        let first = digest_dpkg_configuration(&base, &fragments).unwrap();
        let second = digest_dpkg_configuration(&base, &fragments).unwrap();
        assert_eq!(first, second);

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&base, fragments.join("linked")).unwrap();
            assert!(matches!(
                digest_dpkg_configuration(&base, &fragments),
                Err(AptResolutionCommandError::Unavailable(_))
            ));
        }
    }
}
