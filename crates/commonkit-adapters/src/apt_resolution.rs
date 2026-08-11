use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, Sha256Digest, StableId, digest_domain_json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AptSourceAuthorityV1, ArtifactEvidence, ManagerBindingV1, OfflineInstallRecipeV1,
    PackageFetchRequestV1, PackageObservationV1, PackageResolutionBackend,
    PackageResolutionDraftV1, PackageResolutionError, PackageResolutionProbeV1,
    PackageResolutionRequestV1, PackageTargetV1, ResolvedPackage, SourceBindingV1,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AptInstalledPackageEvidenceV2 {
    name: String,
    architecture: String,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AptLiveSafetyEvidenceV2<'a> {
    live_status_digest: &'a Sha256Digest,
    installed_packages: Vec<AptInstalledPackageEvidenceV2>,
    installed_actions: BTreeSet<(String, String, String)>,
    configured_actions: BTreeSet<(String, String, String)>,
    risks: &'a AptTransactionRisksV1,
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
    pub source_authority: AptSourceAuthorityV1,
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
    pub environment: BTreeMap<String, String>,
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

    pub fn live_safety_command_snapshot(
        request: &AptResolutionSystemRequestV1,
        closure: &[AptResolvedPackageV1],
    ) -> Result<AptCommandSpecV1, AptResolutionCommandError> {
        validate_system_request(request)?;
        apt_live_safety_command(closure, "<private>")
    }

    pub fn package_metadata_command_snapshot(
        closure: &[AptResolvedPackageV1],
    ) -> Result<Vec<AptCommandSpecV1>, AptResolutionCommandError> {
        closure
            .iter()
            .map(|package| apt_package_metadata_command(package, "<private>"))
            .collect()
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
        let apt_digest = digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-get"))?;
        let apt_cache_digest =
            digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-cache"))?;
        let dpkg_digest = digest_no_follow_regular_file(std::path::Path::new("/usr/bin/dpkg"))?;
        let signing_key = read_trusted_signing_key(&repository.signed_by)?;
        let key_digest = content_digest(&signing_key)?;
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
        let source_line = apt_source_line_with_key(
            target,
            canonical_repository,
            repository,
            &format!("{workspace_text}/archive-keyring.gpg"),
        )?;
        let sandbox_identity = current_apt_sandbox_identity()?;
        write_private_apt_workspace_for_identity(
            workspace.path(),
            &source_line,
            &signing_key,
            &[],
            sandbox_identity,
        )?;
        let commands = [
            AptCommandSpecV1 {
                executable: "/usr/bin/apt-get".into(),
                args: vec!["--version".into()],
                environment: apt_command_environment(workspace_text),
                network: false,
            },
            AptCommandSpecV1 {
                executable: "/usr/bin/dpkg".into(),
                args: vec!["--version".into()],
                environment: apt_command_environment(workspace_text),
                network: false,
            },
            AptCommandSpecV1 {
                executable: "/usr/bin/dpkg".into(),
                args: vec!["--print-architecture".into()],
                environment: apt_command_environment(workspace_text),
                network: false,
            },
            AptCommandSpecV1 {
                executable: "/usr/bin/apt-config".into(),
                args: vec!["dump".into()],
                environment: apt_command_environment(workspace_text),
                network: false,
            },
        ];
        let outputs = commands
            .iter()
            .map(|command| run_fixed_command(command, workspace_text))
            .collect::<Result<Vec<_>, _>>()?;
        let architecture = outputs[2].trim();
        if architecture != target.arch {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg architecture differs from the bound target".into(),
            ));
        }
        validate_effective_apt_config(&outputs[3])?;
        if sandbox_identity.is_some() {
            validate_root_apt_sandbox_config(&outputs[3])?;
        }
        let normalized_config = outputs[3].replace(workspace_text, "<private>");
        let normalized_source_line = apt_source_line_with_key(
            target,
            canonical_repository,
            repository,
            "<private>/archive-keyring.gpg",
        )?;
        let binding = manager_binding_from_evidence(
            &outputs[0],
            &outputs[1],
            &normalized_config,
            &normalized_source_line,
            architecture,
            &os_release,
            &apt_digest,
            &apt_cache_digest,
            &dpkg_digest,
            &key_digest,
            &dpkg_config_digest,
        )?;
        if content_digest(&read_trusted_signing_key(&repository.signed_by)?)? != key_digest {
            return Err(AptResolutionCommandError::Failed(
                "APT signing key changed during authority probe".into(),
            ));
        }
        if digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-get"))? != apt_digest
            || digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-cache"))?
                != apt_cache_digest
            || digest_no_follow_regular_file(std::path::Path::new("/usr/bin/dpkg"))? != dpkg_digest
        {
            return Err(AptResolutionCommandError::Failed(
                "APT executable changed during authority probe".into(),
            ));
        }
        Ok(binding)
    }

    pub fn probe_source_authority(
        target: &PackageTargetV1,
        canonical_repository: &str,
        repository: &AptRepositoryConfigurationV1,
    ) -> Result<AptSourceAuthorityV1, AptResolutionCommandError> {
        validate_probe_request(target, canonical_repository, repository)?;
        if !cfg!(target_os = "linux") {
            return Err(AptResolutionCommandError::Unavailable(
                "the production APT resolver is supported only on Linux".into(),
            ));
        }
        let signing_key = read_trusted_signing_key(&repository.signed_by)?;
        Ok(AptSourceAuthorityV1 {
            suite: repository.suite.clone(),
            components: repository.components.clone(),
            signing_authority: repository.signing_authority.clone(),
            signing_key_digest: content_digest(&signing_key)?,
        })
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
        let source_authority = request
            .apt_source_authority
            .filter(|authority| {
                authority.suite == self.repository.suite
                    && authority.components == self.repository.components
                    && authority.signing_authority == self.repository.signing_authority
            })
            .ok_or(PackageResolutionError::InvalidAptRequest)?;
        Ok(AptResolutionSystemRequestV1 {
            declaration: declaration.clone(),
            target: request.target.clone(),
            manager: request.manager.clone(),
            source_id: request.source_id.clone(),
            canonical_repository: request.canonical_repository.into(),
            registry_definition_digest: request.registry_definition_digest.clone(),
            source_authority: source_authority.clone(),
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
        || snapshot.signed_metadata.iter().any(|evidence| {
            evidence.authority != request.repository.signing_authority
                || evidence.signature_digest != request.source_authority.signing_key_digest
        })
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
        || request.source_authority.suite != request.repository.suite
        || request.source_authority.components != request.repository.components
        || request.source_authority.signing_authority != request.repository.signing_authority
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
    let apt_options = apt_options(workspace);
    let apt = |mut args: Vec<String>| {
        let mut all = apt_options.clone();
        all.append(&mut args);
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-get".into(),
            args: all,
            environment: apt_command_environment(workspace),
            network: false,
        }
    };
    let apt_cache = |mut args: Vec<String>| {
        let mut all = apt_options.clone();
        all.append(&mut args);
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-cache".into(),
            args: all,
            environment: apt_command_environment(workspace),
            network: false,
        }
    };
    vec![
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-get".into(),
            args: vec!["--version".into()],
            environment: apt_command_environment(workspace),
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/dpkg".into(),
            args: vec!["--version".into()],
            environment: apt_command_environment(workspace),
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/dpkg".into(),
            args: vec!["--print-architecture".into()],
            environment: apt_command_environment(workspace),
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-config".into(),
            args: vec!["dump".into()],
            environment: apt_command_environment(workspace),
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/dpkg-query".into(),
            args: vec![
                "-W".into(),
                "-f=${binary:Package}\\t${db:Status-Abbrev}\\t${Version}\\t${Architecture}\\n"
                    .into(),
            ],
            environment: apt_command_environment(workspace),
            network: false,
        },
        AptCommandSpecV1 {
            executable: "/usr/bin/apt-mark".into(),
            args: vec!["showhold".into()],
            environment: apt_command_environment(workspace),
            network: false,
        },
        {
            let mut command = apt(vec!["update".into()]);
            command.network = true;
            command
        },
        apt_cache(vec![
            "-o".into(),
            format!("Dir::State::status={workspace}/status"),
            "depends".into(),
            "--recurse".into(),
            exact.clone(),
        ]),
        apt(vec![
            "-o".into(),
            format!("Dir::State::status={workspace}/status"),
            "--simulate".into(),
            "--no-remove".into(),
            "--no-install-recommends".into(),
            "install".into(),
            exact.clone(),
        ]),
        apt(vec![
            "-o".into(),
            format!("Dir::State::status={workspace}/status"),
            "--quiet=2".into(),
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

fn apt_options(workspace: &str) -> Vec<String> {
    vec![
        "-o".into(),
        format!("Dir::Etc::sourcelist={workspace}/sources.list"),
        "-o".into(),
        "Dir::Etc::sourceparts=-".into(),
        "-o".into(),
        format!("Dir::State::lists={workspace}/lists"),
        "-o".into(),
        format!("Dir::Cache::archives={workspace}/archives"),
        "-o".into(),
        format!("Dir::Cache::pkgcache={workspace}/pkgcache.bin"),
        "-o".into(),
        format!("Dir::Cache::srcpkgcache={workspace}/srcpkgcache.bin"),
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
    ]
}

fn apt_live_safety_command(
    closure: &[AptResolvedPackageV1],
    workspace: &str,
) -> Result<AptCommandSpecV1, AptResolutionCommandError> {
    let mut pinned = closure.to_vec();
    pinned.sort_by(|left, right| {
        (&left.name, &left.architecture, &left.version).cmp(&(
            &right.name,
            &right.architecture,
            &right.version,
        ))
    });
    if pinned.is_empty()
        || pinned.windows(2).any(|pair| {
            (&pair[0].name, &pair[0].architecture) == (&pair[1].name, &pair[1].architecture)
        })
        || pinned.iter().any(|package| {
            !safe_apt_token(&package.name)
                || !safe_apt_token(&package.architecture)
                || !safe_apt_version(&package.version)
        })
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "live APT safety closure is empty, duplicate, or malformed".into(),
        ));
    }
    let mut args = apt_options(workspace);
    args.extend([
        "-o".into(),
        format!("Dir::State::status={workspace}/live-status"),
        "--simulate".into(),
        "--no-remove".into(),
        "--no-install-recommends".into(),
        "install".into(),
    ]);
    args.extend(pinned.into_iter().map(|package| {
        format!(
            "{}:{}={}",
            package.name, package.architecture, package.version
        )
    }));
    Ok(AptCommandSpecV1 {
        executable: "/usr/bin/apt-get".into(),
        args,
        environment: apt_command_environment(workspace),
        network: false,
    })
}

fn apt_package_metadata_command(
    package: &AptResolvedPackageV1,
    workspace: &str,
) -> Result<AptCommandSpecV1, AptResolutionCommandError> {
    if !safe_apt_token(&package.name)
        || !safe_apt_token(&package.architecture)
        || !safe_apt_version(&package.version)
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT package metadata request identity is invalid".into(),
        ));
    }
    let mut args = apt_options(workspace);
    args.extend([
        "-o".into(),
        format!("Dir::State::status={workspace}/status"),
        "show".into(),
        format!(
            "{}:{}={}",
            package.name, package.architecture, package.version
        ),
    ]);
    Ok(AptCommandSpecV1 {
        executable: "/usr/bin/apt-cache".into(),
        args,
        environment: apt_command_environment(workspace),
        network: false,
    })
}

fn apt_command_environment(workspace: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("APT_CONFIG".into(), format!("{workspace}/apt.conf"))])
}

fn private_apt_config(workspace: &str) -> String {
    format!(
        concat!(
            "Dir::Etc \"{0}/etc/apt\";\n",
            "Dir::Etc::main \"{0}/empty-main\";\n",
            "Dir::Etc::parts \"{0}/empty-parts\";\n",
            "Dir::Etc::sourcelist \"{0}/sources.list\";\n",
            "Dir::Etc::sourceparts \"-\";\n",
            "Dir::Bin::methods \"/usr/lib/apt/methods\";\n",
            "Dir::Bin::solvers \"/usr/lib/apt/solvers\";\n",
            "Dir::Bin::planners \"/usr/lib/apt/planners\";\n",
            "Dir::Bin::dpkg \"/usr/bin/dpkg\";\n",
            "Dir::Bin::apt-helper \"/usr/lib/apt/apt-helper\";\n",
            "Dir::Bin::gpgv \"/usr/bin/gpgv\";\n",
            "APT::Sandbox::User \"_apt\";\n",
            "#clear DPkg::Pre-Invoke;\n",
            "#clear DPkg::Post-Invoke;\n",
            "#clear APT::Update::Pre-Invoke;\n",
            "#clear APT::Update::Post-Invoke;\n",
            "#clear Acquire::http::Proxy-Auto-Detect;\n",
            "#clear Acquire::https::Proxy-Auto-Detect;\n",
        ),
        workspace
    )
}

fn validate_effective_apt_config(config: &str) -> Result<(), AptResolutionCommandError> {
    const FIXED_HELPERS: [(&str, &str); 7] = [
        ("dir::bin::methods", "/usr/lib/apt/methods"),
        ("dir::bin::solvers", "/usr/lib/apt/solvers"),
        ("dir::bin::planners", "/usr/lib/apt/planners"),
        ("dir::bin::dpkg", "/usr/bin/dpkg"),
        ("dir::bin::apt-helper", "/usr/lib/apt/apt-helper"),
        ("dir::bin::gpgv", "/usr/bin/gpgv"),
        ("apt::sandbox::user", "_apt"),
    ];
    for line in config
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let lower = line.to_ascii_lowercase();
        if lower.contains("::pre-invoke")
            || lower.contains("::post-invoke")
            || lower.contains("proxy-auto-detect")
            || lower.starts_with("apt::solver")
            || lower.starts_with("apt::planner")
            || lower.starts_with("apt::get::solver")
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "effective APT configuration contains a hook or automatic proxy".into(),
            ));
        }
        for (key, expected) in FIXED_HELPERS {
            if lower.starts_with(key) && !line.contains(&format!("\"{expected}\"")) {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "effective APT helper path differs from the fixed system capability".into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_root_apt_sandbox_config(config: &str) -> Result<(), AptResolutionCommandError> {
    let sandbox = config
        .lines()
        .map(str::trim)
        .filter(|line| line.to_ascii_lowercase().starts_with("apt::sandbox::user"))
        .collect::<Vec<_>>();
    if sandbox.len() != 1 || !sandbox[0].contains("\"_apt\"") {
        return Err(AptResolutionCommandError::InvalidOutput(
            "effective root APT configuration lacks the required _apt sandbox identity".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_trusted_key_for_uid(
    path: &std::path::Path,
    expected_uid: u32,
) -> Result<Vec<u8>, AptResolutionCommandError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if !metadata.is_file() || metadata.uid() != expected_uid || metadata.mode() & 0o022 != 0 {
        return Err(AptResolutionCommandError::Unavailable(
            "APT signing key is not a trusted owner-bound non-writable regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if bytes.is_empty() {
        return Err(AptResolutionCommandError::Unavailable(
            "APT signing key is empty".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_trusted_key_for_uid(
    _path: &std::path::Path,
    _expected_uid: u32,
) -> Result<Vec<u8>, AptResolutionCommandError> {
    Err(AptResolutionCommandError::Unavailable(
        "trusted APT key snapshots require Unix no-follow file capabilities".into(),
    ))
}

fn read_trusted_signing_key(path: &std::path::Path) -> Result<Vec<u8>, AptResolutionCommandError> {
    read_trusted_key_for_uid(path, 0)
}

#[cfg(unix)]
fn read_trusted_dpkg_status() -> Result<Vec<u8>, AptResolutionCommandError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let path = std::path::Path::new("/var/lib/dpkg/status");
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(AptResolutionCommandError::Unavailable(
            "dpkg status is not a trusted root-owned regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
        return Err(AptResolutionCommandError::Unavailable(
            "dpkg status is empty or exceeds the fixed observation bound".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_trusted_dpkg_status() -> Result<Vec<u8>, AptResolutionCommandError> {
    Err(AptResolutionCommandError::Unavailable(
        "trusted dpkg status snapshots require Unix no-follow file capabilities".into(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AptSandboxIdentityV1 {
    uid: u32,
    gid: u32,
}

fn select_apt_sandbox_identity(
    effective_uid: u32,
    passwd: &str,
) -> Result<Option<AptSandboxIdentityV1>, AptResolutionCommandError> {
    if effective_uid != 0 {
        return Ok(None);
    }
    let matches = passwd
        .lines()
        .filter_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            (fields.len() >= 7 && fields[0] == "_apt").then_some(fields)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(AptResolutionCommandError::Unavailable(
            "root APT resolution requires exactly one _apt sandbox identity".into(),
        ));
    }
    let uid = matches[0][2].parse::<u32>().map_err(|_| {
        AptResolutionCommandError::Unavailable("_apt sandbox uid is invalid".into())
    })?;
    let gid = matches[0][3].parse::<u32>().map_err(|_| {
        AptResolutionCommandError::Unavailable("_apt sandbox gid is invalid".into())
    })?;
    if uid == 0 {
        return Err(AptResolutionCommandError::Unavailable(
            "_apt sandbox identity must not be root".into(),
        ));
    }
    Ok(Some(AptSandboxIdentityV1 { uid, gid }))
}

#[cfg(unix)]
fn current_apt_sandbox_identity() -> Result<Option<AptSandboxIdentityV1>, AptResolutionCommandError>
{
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let effective_uid = unsafe { libc::geteuid() };
    if effective_uid != 0 {
        return Ok(None);
    }
    let path = std::path::Path::new("/etc/passwd");
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(AptResolutionCommandError::Unavailable(
            "APT sandbox identity source is not a trusted root-owned regular file".into(),
        ));
    }
    let mut passwd = String::new();
    file.read_to_string(&mut passwd)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    select_apt_sandbox_identity(effective_uid, &passwd)
}

#[cfg(not(unix))]
fn current_apt_sandbox_identity() -> Result<Option<AptSandboxIdentityV1>, AptResolutionCommandError>
{
    Ok(None)
}

fn write_private_apt_workspace_for_identity(
    workspace: &std::path::Path,
    source_line: &str,
    signing_key: &[u8],
    live_status: &[u8],
    identity: Option<AptSandboxIdentityV1>,
) -> Result<(), AptResolutionCommandError> {
    for directory in [
        workspace.join("etc/apt"),
        workspace.join("empty-parts"),
        workspace.join("lists/partial"),
        workspace.join("archives/partial"),
    ] {
        fs::create_dir_all(directory)
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    }
    let workspace_text = workspace.to_str().ok_or_else(|| {
        AptResolutionCommandError::Unavailable("private APT path is not UTF-8".into())
    })?;
    fs::write(
        workspace.join("apt.conf"),
        private_apt_config(workspace_text),
    )
    .and_then(|_| fs::write(workspace.join("empty-main"), []))
    .and_then(|_| fs::write(workspace.join("sources.list"), source_line))
    .and_then(|_| fs::write(workspace.join("status"), []))
    .and_then(|_| fs::write(workspace.join("live-status"), live_status))
    .and_then(|_| fs::write(workspace.join("archive-keyring.gpg"), signing_key))
    .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    #[cfg(unix)]
    configure_private_apt_workspace_permissions(workspace, identity)?;
    Ok(())
}

#[cfg(unix)]
fn configure_private_apt_workspace_permissions(
    workspace: &std::path::Path,
    identity: Option<AptSandboxIdentityV1>,
) -> Result<(), AptResolutionCommandError> {
    use std::os::unix::fs::{PermissionsExt, chown};

    let set_mode = |path: &std::path::Path, mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))
    };
    let files = [
        "apt.conf",
        "empty-main",
        "sources.list",
        "status",
        "live-status",
        "archive-keyring.gpg",
    ];
    let Some(identity) = identity else {
        set_mode(workspace, 0o700)?;
        for directory in [
            "etc",
            "etc/apt",
            "empty-parts",
            "lists",
            "lists/partial",
            "archives",
            "archives/partial",
        ] {
            set_mode(&workspace.join(directory), 0o700)?;
        }
        for file in files {
            set_mode(
                &workspace.join(file),
                if file == "archive-keyring.gpg" {
                    0o400
                } else {
                    0o600
                },
            )?;
        }
        return Ok(());
    };
    for directory in [
        workspace,
        &workspace.join("etc"),
        &workspace.join("etc/apt"),
        &workspace.join("empty-parts"),
        &workspace.join("lists"),
        &workspace.join("archives"),
    ] {
        chown(directory, None, Some(identity.gid))
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        set_mode(directory, 0o750)?;
    }
    for partial in [
        workspace.join("lists/partial"),
        workspace.join("archives/partial"),
    ] {
        chown(&partial, Some(identity.uid), Some(identity.gid))
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        set_mode(&partial, 0o700)?;
    }
    for file in [
        "apt.conf",
        "empty-main",
        "sources.list",
        "archive-keyring.gpg",
    ] {
        let path = workspace.join(file);
        chown(&path, None, Some(identity.gid))
            .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
        set_mode(&path, 0o440)?;
    }
    for file in ["status", "live-status"] {
        set_mode(&workspace.join(file), 0o600)?;
    }
    Ok(())
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

    let key_bytes = read_trusted_signing_key(&request.repository.signed_by)?;
    let live_status_bytes = read_trusted_dpkg_status()?;
    let apt_digest = digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-get"))?;
    let apt_cache_digest =
        digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-cache"))?;
    let dpkg_digest = digest_no_follow_regular_file(std::path::Path::new("/usr/bin/dpkg"))?;
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
    let workspace_text = workspace_path.to_str().ok_or_else(|| {
        AptResolutionCommandError::Unavailable("private APT path is not UTF-8".into())
    })?;
    let source_line = apt_source_line_with_key(
        &request.target,
        &request.canonical_repository,
        &request.repository,
        &format!("{workspace_text}/archive-keyring.gpg"),
    )?;
    let normalized_source_line = apt_source_line_with_key(
        &request.target,
        &request.canonical_repository,
        &request.repository,
        "<private>/archive-keyring.gpg",
    )?;
    let sandbox_identity = current_apt_sandbox_identity()?;
    write_private_apt_workspace_for_identity(
        workspace_path,
        &source_line,
        &key_bytes,
        &live_status_bytes,
        sandbox_identity,
    )?;
    if key_digest != request.source_authority.signing_key_digest {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT signing key differs from the source-registry authority".into(),
        ));
    }
    let commands = apt_command_plan(request, workspace_text);
    let mut outputs = vec![String::new(); commands.len()];
    for (index, command) in commands.iter().enumerate().take(6) {
        outputs[index] = run_fixed_command(command, workspace_text)?;
    }

    let architecture = outputs[2].trim();
    if architecture != request.target.arch {
        return Err(AptResolutionCommandError::InvalidOutput(
            "dpkg architecture differs from the bound target".into(),
        ));
    }
    validate_effective_apt_config(&outputs[3])?;
    if sandbox_identity.is_some() {
        validate_root_apt_sandbox_config(&outputs[3])?;
    }
    let normalized_config = outputs[3].replace(workspace_text, "<private>");
    let manager = manager_binding_from_evidence(
        &outputs[0],
        &outputs[1],
        &normalized_config,
        &normalized_source_line,
        architecture,
        &os_release,
        &apt_digest,
        &apt_cache_digest,
        &dpkg_digest,
        &key_digest,
        &dpkg_config_digest,
    )?;
    if manager != request.manager {
        return Err(AptResolutionCommandError::InvalidOutput(
            "local APT manager authority differs from the controller binding".into(),
        ));
    }
    let live_status = std::str::from_utf8(&live_status_bytes)
        .map_err(|_| AptResolutionCommandError::InvalidOutput("dpkg status is not UTF-8".into()))?;
    let query_output = outputs[4].clone();
    let (installed, ()) = with_complete_installed_state(live_status, &query_output, |_| {
        for (index, command) in commands.iter().enumerate().skip(6) {
            outputs[index] = run_fixed_command(command, workspace_text)?;
        }
        Ok(())
    })?;

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
    let archives = resolve_archives_from_package_metadata(
        &request.canonical_repository,
        &outputs[8],
        &outputs[9],
        |package| {
            let command = apt_package_metadata_command(package, workspace_text)?;
            run_fixed_command(&command, workspace_text)
        },
    )?;
    validate_simulated_archive_closure(&outputs[8], &archives)?;
    let mut risks = parse_solver_risks(&outputs[8], &outputs[7], &outputs[5]);
    let closure_names = archives
        .iter()
        .map(|archive| archive.package_name.as_str())
        .collect::<BTreeSet<_>>();
    risks
        .held
        .retain(|package| closure_names.contains(package.as_str()));
    risks.downgrades = find_downgrades(&archives, &installed, workspace_text)?;
    let closure = archives
        .iter()
        .map(|archive| AptResolvedPackageV1 {
            name: archive.package_name.clone(),
            version: archive.version.clone(),
            architecture: archive.architecture.clone(),
        })
        .collect::<Vec<_>>();
    let live_safety_command = apt_live_safety_command(&closure, workspace_text)?;
    let live_safety_output = run_fixed_command(&live_safety_command, workspace_text)?;
    validate_live_safety_simulation(&live_safety_output, &closure)?;
    let live_risks = parse_solver_risks(&live_safety_output, "", "");
    risks.removals.extend(live_risks.removals);
    risks.replacements.extend(live_risks.replacements);
    risks
        .unresolved_alternatives
        .extend(live_risks.unresolved_alternatives);
    let live_status_digest = content_digest(&live_status_bytes)?;
    let live_safety_digest = apt_live_safety_evidence_digest(
        &live_status_digest,
        installed
            .iter()
            .map(|((name, architecture), version)| {
                (name.clone(), architecture.clone(), version.clone())
            })
            .collect(),
        &live_safety_output,
        &risks,
    )?;
    let mut installed_versions = installed
        .iter()
        .map(|((name, architecture), version)| format!("{name}:{architecture}={version}"))
        .collect::<BTreeSet<_>>();
    installed_versions.insert(format!(
        "commonkit-apt-live-safety={}",
        live_safety_digest.as_str()
    ));
    let before = PackageObservationV1 { installed_versions };

    if digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-get"))? != apt_digest
        || digest_no_follow_regular_file(std::path::Path::new("/usr/bin/apt-cache"))?
            != apt_cache_digest
        || digest_no_follow_regular_file(std::path::Path::new("/usr/bin/dpkg"))? != dpkg_digest
        || content_digest(&read_trusted_signing_key(&request.repository.signed_by)?)?
            != signed_metadata[0].signature_digest
        || digest_dpkg_configuration(
            std::path::Path::new("/etc/dpkg/dpkg.cfg"),
            std::path::Path::new("/etc/dpkg/dpkg.cfg.d"),
        )? != dpkg_config_digest
        || content_digest(&read_trusted_dpkg_status()?)? != live_status_digest
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
    _repository: &AptRepositoryConfigurationV1,
) -> Result<(), AptResolutionCommandError> {
    for path in [
        PathBuf::from("/usr/bin/apt-get"),
        PathBuf::from("/usr/bin/apt-config"),
        PathBuf::from("/usr/bin/apt-cache"),
        PathBuf::from("/usr/bin/dpkg"),
        PathBuf::from("/usr/bin/dpkg-query"),
        PathBuf::from("/usr/bin/apt-mark"),
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

fn apt_source_line_with_key(
    target: &PackageTargetV1,
    canonical_repository: &str,
    repository: &AptRepositoryConfigurationV1,
    signed_by: &str,
) -> Result<String, AptResolutionCommandError> {
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

fn safe_apt_version(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '+' | '-' | '_' | ':' | '~')
        })
}

fn valid_apt_repository(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && !url.cannot_be_a_base()
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !value.contains('%')
        && url.as_str().trim_end_matches('/') == value.trim_end_matches('/')
}

fn run_fixed_command(
    command: &AptCommandSpecV1,
    workspace: &str,
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
        .envs(&command.environment)
        .output()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if output.stdout.len() > MAX_OUTPUT_BYTES || output.stderr.len() > MAX_OUTPUT_BYTES {
        return Err(AptResolutionCommandError::Failed(
            "APT command output exceeded the fixed bound".into(),
        ));
    }
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    validate_apt_command_diagnostics(&diagnostics)?;
    if !output.status.success() {
        return Err(AptResolutionCommandError::Failed(
            diagnostics.chars().take(1_024).collect(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| AptResolutionCommandError::InvalidOutput("APT output is not UTF-8".into()))
}

fn validate_apt_command_diagnostics(stderr: &str) -> Result<(), AptResolutionCommandError> {
    if stderr.to_ascii_lowercase().contains("unsandboxed as root") {
        return Err(AptResolutionCommandError::Failed(
            "APT refused the required _apt download sandbox".into(),
        ));
    }
    Ok(())
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
    apt_cache_digest: &Sha256Digest,
    dpkg_digest: &Sha256Digest,
    key_digest: &Sha256Digest,
    dpkg_config_digest: &Sha256Digest,
) -> Result<ManagerBindingV1, AptResolutionCommandError> {
    let apt_version = parse_tool_version(apt_version_output, "apt")?;
    let dpkg_version = parse_tool_version(dpkg_version_output, "version")?;
    let executable_digest = digest_domain_json(
        "commonkit.apt-executable-binding.v1",
        &(apt_digest, apt_cache_digest, dpkg_digest),
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

type InstalledPackageState = BTreeMap<(String, String), String>;

fn apt_live_safety_evidence_digest(
    live_status_digest: &Sha256Digest,
    mut installed: Vec<(String, String, String)>,
    simulation: &str,
    risks: &AptTransactionRisksV1,
) -> Result<Sha256Digest, AptResolutionCommandError> {
    installed.sort();
    let mut identities = BTreeSet::new();
    let mut installed_packages = Vec::with_capacity(installed.len());
    for (name, architecture, version) in installed {
        if !safe_apt_token(&name) || !safe_apt_token(&architecture) || !safe_apt_version(&version) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT live-safety installed identity is malformed".into(),
            ));
        }
        if !identities.insert((name.clone(), architecture.clone())) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT live-safety installed identity is duplicated".into(),
            ));
        }
        installed_packages.push(AptInstalledPackageEvidenceV2 {
            name,
            architecture,
            version,
        });
    }
    let evidence = AptLiveSafetyEvidenceV2 {
        live_status_digest,
        installed_packages,
        installed_actions: parse_simulated_packages(simulation)?,
        configured_actions: parse_configured_packages(simulation)?,
        risks,
    };
    digest_domain_json("commonkit.apt-live-safety-evidence.v2", &evidence)
        .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))
}

fn parse_complete_installed_state(
    status: &str,
    query: &str,
) -> Result<InstalledPackageState, AptResolutionCommandError> {
    let status_packages = parse_status_installed_packages(status)?;
    let query_packages = parse_query_installed_packages(query)?;
    if status_packages != query_packages {
        return Err(AptResolutionCommandError::InvalidOutput(
            "dpkg installed-state observations are incomplete or inconsistent".into(),
        ));
    }
    Ok(status_packages)
}

fn with_complete_installed_state<T>(
    status: &str,
    query: &str,
    continue_after_observation: impl FnOnce(
        &InstalledPackageState,
    ) -> Result<T, AptResolutionCommandError>,
) -> Result<(InstalledPackageState, T), AptResolutionCommandError> {
    let installed = parse_complete_installed_state(status, query)?;
    let result = continue_after_observation(&installed)?;
    Ok((installed, result))
}

fn parse_status_installed_packages(
    status: &str,
) -> Result<BTreeMap<(String, String), String>, AptResolutionCommandError> {
    let mut installed = BTreeMap::new();
    for paragraph in status
        .split("\n\n")
        .filter(|value| !value.trim().is_empty())
    {
        let mut fields = BTreeMap::new();
        for line in paragraph.lines() {
            if line.starts_with([' ', '\t']) {
                continue;
            }
            let (name, value) = line.split_once(':').ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput("malformed dpkg status record".into())
            })?;
            if fields.insert(name, value.trim()).is_some() {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "duplicate dpkg status field".into(),
                ));
            }
        }
        let package = fields.get("Package").copied().ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("dpkg status record lacks Package".into())
        })?;
        let package_status = fields.get("Status").copied().ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("dpkg status record lacks Status".into())
        })?;
        let status_fields = package_status.split_whitespace().collect::<Vec<_>>();
        if status_fields.len() != 3 {
            return Err(AptResolutionCommandError::InvalidOutput(
                "malformed dpkg package status".into(),
            ));
        }
        if !matches!(
            status_fields[0],
            "unknown" | "install" | "hold" | "deinstall" | "purge"
        ) || !matches!(status_fields[1], "ok" | "reinstreq")
            || !matches!(
                status_fields[2],
                "not-installed"
                    | "config-files"
                    | "half-installed"
                    | "unpacked"
                    | "half-configured"
                    | "triggers-awaited"
                    | "triggers-pending"
                    | "installed"
            )
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg status contains an unknown package state".into(),
            ));
        }
        if status_fields[1] != "ok" {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg package is in an error state".into(),
            ));
        }
        if matches!(status_fields[2], "not-installed" | "config-files") {
            continue;
        }
        if status_fields[2] != "installed" {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg package is in a nonterminal state".into(),
            ));
        }
        let architecture = fields.get("Architecture").copied().ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "installed dpkg status record lacks Architecture".into(),
            )
        })?;
        let version = fields.get("Version").copied().ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "installed dpkg status record lacks Version".into(),
            )
        })?;
        insert_installed_package(&mut installed, package, architecture, version)?;
    }
    if installed.is_empty() {
        return Err(AptResolutionCommandError::InvalidOutput(
            "dpkg status contains no installed packages".into(),
        ));
    }
    Ok(installed)
}

fn parse_query_installed_packages(
    query: &str,
) -> Result<BTreeMap<(String, String), String>, AptResolutionCommandError> {
    let mut installed = BTreeMap::new();
    for line in query.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 4 || fields[1].len() != 3 || !fields[1].is_ascii() {
            return Err(AptResolutionCommandError::InvalidOutput(
                "malformed dpkg-query installed-state record".into(),
            ));
        }
        let status = fields[1].as_bytes();
        if !b"uihpr".contains(&status[0])
            || !b"ncHUFWti".contains(&status[1])
            || !matches!(status[2], b' ' | b'R')
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg-query returned an unknown package state".into(),
            ));
        }
        if status[1] != b'i' {
            continue;
        }
        if status[2] != b' ' {
            return Err(AptResolutionCommandError::InvalidOutput(
                "installed dpkg-query package is in an error state".into(),
            ));
        }
        let (name, qualified_architecture) = fields[0]
            .split_once(':')
            .map_or((fields[0], None), |(name, architecture)| {
                (name, Some(architecture))
            });
        if qualified_architecture.is_some_and(|architecture| architecture != fields[3]) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "dpkg-query package and architecture disagree".into(),
            ));
        }
        insert_installed_package(&mut installed, name, fields[3], fields[2])?;
    }
    if installed.is_empty() {
        return Err(AptResolutionCommandError::InvalidOutput(
            "dpkg-query returned no installed packages".into(),
        ));
    }
    Ok(installed)
}

fn insert_installed_package(
    installed: &mut BTreeMap<(String, String), String>,
    name: &str,
    architecture: &str,
    version: &str,
) -> Result<(), AptResolutionCommandError> {
    if !safe_apt_token(name) || !safe_apt_token(architecture) || !safe_apt_version(version) {
        return Err(AptResolutionCommandError::InvalidOutput(
            "dpkg installed-state identity is malformed".into(),
        ));
    }
    if installed
        .insert(
            (name.to_owned(), architecture.to_owned()),
            version.to_owned(),
        )
        .is_some()
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "duplicate dpkg installed-state package".into(),
        ));
    }
    Ok(())
}

fn find_downgrades(
    archives: &[AptResolvedArchiveV1],
    installed: &BTreeMap<(String, String), String>,
    workspace: &str,
) -> Result<BTreeSet<String>, AptResolutionCommandError> {
    let mut downgrades = BTreeSet::new();
    for archive in archives {
        let Some(old_version) =
            installed.get(&(archive.package_name.clone(), archive.architecture.clone()))
        else {
            continue;
        };
        let spec = AptCommandSpecV1 {
            executable: "/usr/bin/dpkg".into(),
            args: vec![
                "--compare-versions".into(),
                archive.version.clone(),
                "lt".into(),
                old_version.clone(),
            ],
            environment: apt_command_environment(workspace),
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
                downgrades.insert(archive.package_name.clone());
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

fn digest_no_follow_regular_file(
    path: &std::path::Path,
) -> Result<Sha256Digest, AptResolutionCommandError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(path)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    if !metadata.is_file() {
        return Err(AptResolutionCommandError::Unavailable(format!(
            "required executable is not a no-follow regular file: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| AptResolutionCommandError::Unavailable(error.to_string()))?;
    content_digest(&bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AptPrintUriRecord {
    immutable_locator: String,
    filename: String,
    size: u64,
    reported_sha256: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AptPackageMetadataRecord {
    package: AptResolvedPackageV1,
    filename: String,
    size: u64,
    sha256: Sha256Digest,
}

fn parse_print_uri_records(
    output: &str,
) -> Result<Vec<AptPrintUriRecord>, AptResolutionCommandError> {
    let mut records = Vec::new();
    let mut locators = BTreeSet::new();
    let mut filenames = BTreeSet::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !line.starts_with('\'') {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT print-uris output contains an unknown nonempty line".into(),
            ));
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
        let filename = unquote_apt_field(fields[1]).ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "APT archive filename quoting is invalid".into(),
            )
        })?;
        let uri_filename = locator.rsplit('/').next().unwrap_or_default();
        if filename.is_empty()
            || !filename.ends_with(".deb")
            || filename.contains('/')
            || filename != uri_filename
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT archive filename and URI basename disagree".into(),
            ));
        }
        let size = fields[2].parse::<u64>().map_err(|_| {
            AptResolutionCommandError::InvalidOutput("APT archive size is invalid".into())
        })?;
        if size == 0 {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT archive size is zero".into(),
            ));
        }
        let reported_sha256 = if let Some(checksum) = fields[3].strip_prefix("SHA256:") {
            Some(
                Sha256Digest::parse(format!("sha256:{checksum}"))
                    .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))?,
            )
        } else if let Some(checksum) = fields[3].strip_prefix("MD5Sum:") {
            if checksum.len() != 32
                || !checksum.chars().all(|character| {
                    character.is_ascii_hexdigit() && !character.is_ascii_uppercase()
                })
            {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "APT archive MD5 diagnostic is malformed".into(),
                ));
            }
            None
        } else {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT archive reported an unsupported checksum kind".into(),
            ));
        };
        if !locators.insert(locator) || !filenames.insert(filename) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT returned duplicate archive URI records".into(),
            ));
        }
        records.push(AptPrintUriRecord {
            immutable_locator: locator.into(),
            filename: filename.into(),
            size,
            reported_sha256,
        });
    }
    if records.is_empty() {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT returned no exact archive records".into(),
        ));
    }
    records.sort_by(|left, right| left.filename.cmp(&right.filename));
    Ok(records)
}

fn unquote_apt_field(value: &str) -> Option<&str> {
    match (value.strip_prefix('\''), value.strip_suffix('\'')) {
        (Some(without_prefix), Some(_)) => without_prefix.strip_suffix('\''),
        (None, None) if !value.contains('\'') => Some(value),
        _ => None,
    }
}

fn parse_package_metadata_record(
    output: &str,
    expected: &AptResolvedPackageV1,
) -> Result<AptPackageMetadataRecord, AptResolutionCommandError> {
    let paragraphs = output
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>();
    if paragraphs.len() != 1 {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT package metadata did not contain exactly one record".into(),
        ));
    }
    let required = [
        "Package",
        "Version",
        "Architecture",
        "Filename",
        "Size",
        "SHA256",
    ];
    let mut fields = BTreeMap::new();
    for raw_line in paragraphs[0].lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let (key, value) = line.split_once(": ").ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "APT package metadata line is malformed".into(),
            )
        })?;
        if required.contains(&key) && fields.insert(key, value).is_some() {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT package metadata contains a duplicate required field".into(),
            ));
        }
    }
    if fields.len() != required.len() || required.iter().any(|key| !fields.contains_key(key)) {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT package metadata lacks a required signed field".into(),
        ));
    }
    if fields["Package"] != expected.name
        || fields["Version"] != expected.version
        || fields["Architecture"] != expected.architecture
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT package metadata identity differs from the exact closure".into(),
        ));
    }
    let filename = fields["Filename"];
    if filename.starts_with('/')
        || filename.contains(['\\', '?', '#', '%'])
        || filename
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        || !filename.ends_with(".deb")
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT package metadata filename is unsafe".into(),
        ));
    }
    let size = fields["Size"].parse::<u64>().map_err(|_| {
        AptResolutionCommandError::InvalidOutput("APT package metadata size is invalid".into())
    })?;
    if size == 0 {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT package metadata size is zero".into(),
        ));
    }
    let sha256 = Sha256Digest::parse(format!("sha256:{}", fields["SHA256"]))
        .map_err(|error| AptResolutionCommandError::InvalidOutput(error.to_string()))?;
    Ok(AptPackageMetadataRecord {
        package: expected.clone(),
        filename: filename.into(),
        size,
        sha256,
    })
}

fn resolve_archives_from_package_metadata(
    canonical_repository: &str,
    simulation: &str,
    print_uris: &str,
    mut load_metadata: impl FnMut(&AptResolvedPackageV1) -> Result<String, AptResolutionCommandError>,
) -> Result<Vec<AptResolvedArchiveV1>, AptResolutionCommandError> {
    let simulated = parse_simulated_packages(simulation)?;
    if simulated.is_empty() {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT simulation returned no exact package closure".into(),
        ));
    }
    let mut print_records = parse_print_uri_records(print_uris)?
        .into_iter()
        .map(|record| (record.filename.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let mut archives = Vec::with_capacity(simulated.len());
    for (name, architecture, version) in simulated {
        let package = AptResolvedPackageV1 {
            name,
            version,
            architecture,
        };
        let metadata = parse_package_metadata_record(&load_metadata(&package)?, &package)?;
        let basename = metadata.filename.rsplit('/').next().unwrap_or_default();
        let print_record = print_records.remove(basename).ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "APT signed package metadata has no matching archive URI".into(),
            )
        })?;
        let expected_locator = format!(
            "{}/{}",
            canonical_repository.trim_end_matches('/'),
            metadata.filename
        );
        if !valid_apt_repository(canonical_repository)
            || print_record.immutable_locator != expected_locator
            || metadata.size != print_record.size
            || print_record
                .reported_sha256
                .as_ref()
                .is_some_and(|checksum| checksum != &metadata.sha256)
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT archive URI and signed package metadata disagree".into(),
            ));
        }
        archives.push(AptResolvedArchiveV1 {
            package_name: metadata.package.name,
            version: metadata.package.version,
            architecture: metadata.package.architecture,
            immutable_locator: print_record.immutable_locator,
            upstream_checksum: metadata.sha256,
            size: metadata.size,
        });
    }
    if !print_records.is_empty() {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT archive URI output contains records outside the exact closure".into(),
        ));
    }
    Ok(archives)
}

fn parse_solver_risks(
    simulation: &str,
    dependency_metadata: &str,
    held: &str,
) -> AptTransactionRisksV1 {
    let mut risks = AptTransactionRisksV1 {
        held: held
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .filter_map(|line| line.split(':').next().map(str::to_owned))
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
    }
    let mut alternative = None;
    for line in dependency_metadata.lines() {
        let trimmed = line.trim();
        if let Some(dependency) = trimmed
            .strip_prefix("|Depends: ")
            .or_else(|| trimmed.strip_prefix("|PreDepends: "))
        {
            alternative = Some(dependency.trim().to_owned());
            continue;
        }
        if let Some(dependency) = trimmed
            .strip_prefix("Depends: ")
            .or_else(|| trimmed.strip_prefix("PreDepends: "))
        {
            if let Some(first) = alternative.take() {
                risks
                    .unresolved_alternatives
                    .insert(format!("{first} | {}", dependency.trim()));
            } else if dependency.starts_with('<') && dependency.ends_with('>') {
                risks.unresolved_alternatives.insert(dependency.to_owned());
            }
        }
        if let Some(package) = trimmed
            .strip_prefix("Replaces: ")
            .or_else(|| trimmed.strip_prefix("Conflicts: "))
            .or_else(|| trimmed.strip_prefix("Breaks: "))
        {
            risks.replacements.insert(package.trim().to_owned());
        }
    }
    if let Some(first) = alternative {
        risks.unresolved_alternatives.insert(first);
    }
    risks
}

fn validate_simulated_archive_closure(
    simulation: &str,
    archives: &[AptResolvedArchiveV1],
) -> Result<(), AptResolutionCommandError> {
    let simulated = parse_simulated_packages(simulation)?;
    let archived = archives
        .iter()
        .map(|archive| {
            (
                archive.package_name.clone(),
                archive.architecture.clone(),
                archive.version.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    if simulated.is_empty() || simulated != archived {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT simulation and archive enumeration disagree on the exact closure".into(),
        ));
    }
    Ok(())
}

fn validate_live_safety_simulation(
    simulation: &str,
    closure: &[AptResolvedPackageV1],
) -> Result<(), AptResolutionCommandError> {
    let risks = parse_solver_risks(simulation, "", "");
    if !risks.removals.is_empty()
        || !risks.replacements.is_empty()
        || !risks.unresolved_alternatives.is_empty()
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "live APT safety simulation requires a destructive transaction".into(),
        ));
    }
    let intended = closure
        .iter()
        .map(|package| {
            (
                package.name.clone(),
                package.architecture.clone(),
                package.version.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    if !parse_simulated_packages(simulation)?.is_subset(&intended)
        || !parse_configured_packages(simulation)?.is_subset(&intended)
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "live APT safety simulation selected a package outside the pinned closure".into(),
        ));
    }
    Ok(())
}

fn parse_simulated_packages(
    simulation: &str,
) -> Result<BTreeSet<(String, String, String)>, AptResolutionCommandError> {
    let mut simulated = BTreeSet::new();
    for line in simulation.lines() {
        if line.split_whitespace().next() == Some("Inst") && line != line.trim() {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT install action contains leading or trailing whitespace".into(),
            ));
        }
        match line.strip_prefix("Inst ") {
            Some(_) => {}
            None if line.split_whitespace().next() == Some("Inst") => {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "malformed APT install action".into(),
                ));
            }
            None => continue,
        };
        let package = parse_simulated_install_line(line)?;
        if !simulated.insert(package) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT simulation returned a duplicate package".into(),
            ));
        }
    }
    Ok(simulated)
}

fn parse_simulated_install_line(
    line: &str,
) -> Result<(String, String, String), AptResolutionCommandError> {
    let rest = line.strip_prefix("Inst ").ok_or_else(|| {
        AptResolutionCommandError::InvalidOutput("malformed APT simulation package".into())
    })?;
    let (name_with_arch, mut description) = rest.split_once(' ').ok_or_else(|| {
        AptResolutionCommandError::InvalidOutput("malformed APT simulation package".into())
    })?;
    if description.starts_with('[') {
        let (current_version, after_current) = take_apt_group(description, '[', ']')?;
        if !safe_apt_version(current_version) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "malformed APT simulation current version".into(),
            ));
        }
        description = after_current.strip_prefix(' ').ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "malformed APT simulation current version group".into(),
            )
        })?;
    }
    let (primary, trailing) = take_apt_group(description, '(', ')')?;
    validate_apt_install_annotations(trailing)?;
    let (version, release_and_architecture) = primary.split_once(' ').ok_or_else(|| {
        AptResolutionCommandError::InvalidOutput("malformed APT simulation version".into())
    })?;
    let (release, architecture) = release_and_architecture
        .rsplit_once(" [")
        .and_then(|(release, architecture)| {
            architecture
                .strip_suffix(']')
                .map(|architecture| (release, architecture))
        })
        .ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "APT simulation did not bind a package architecture".into(),
            )
        })?;
    let (name, qualified_architecture) = name_with_arch
        .split_once(':')
        .map_or((name_with_arch, None), |(name, architecture)| {
            (name, Some(architecture))
        });
    if !safe_apt_token(name)
        || !safe_apt_version(version)
        || !safe_apt_token(architecture)
        || !valid_apt_release_list(release)
        || qualified_architecture.is_some_and(|qualified| qualified != architecture)
    {
        return Err(AptResolutionCommandError::InvalidOutput(
            "APT simulation package identity is malformed".into(),
        ));
    }
    Ok((name.into(), architecture.into(), version.into()))
}

fn take_apt_group(
    input: &str,
    open: char,
    close: char,
) -> Result<(&str, &str), AptResolutionCommandError> {
    if !input.starts_with(open) {
        return Err(AptResolutionCommandError::InvalidOutput(
            "malformed APT simulation group".into(),
        ));
    }
    let end = input[open.len_utf8()..]
        .find(close)
        .map(|index| index + open.len_utf8())
        .ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("unbalanced APT simulation group".into())
        })?;
    let contents = &input[open.len_utf8()..end];
    if contents.contains([open, close]) {
        return Err(AptResolutionCommandError::InvalidOutput(
            "nested APT simulation group".into(),
        ));
    }
    Ok((contents, &input[end + close.len_utf8()..]))
}

fn validate_apt_install_annotations(mut trailing: &str) -> Result<(), AptResolutionCommandError> {
    while !trailing.is_empty() {
        let group = trailing.strip_prefix(' ').ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "malformed APT simulation annotation separator".into(),
            )
        })?;
        let (contents, remainder) = take_apt_group(group, '[', ']')?;
        if contents.is_empty() {
            if !remainder.is_empty() {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "APT empty break annotation is not final".into(),
                ));
            }
        } else if let Some((package, dependency)) = contents.split_once(" on ") {
            if dependency.contains(" on ")
                || !valid_apt_simulation_package(package)
                || !valid_apt_simulation_package(dependency)
            {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "malformed APT dependency annotation".into(),
                ));
            }
        } else {
            let short_breaks = contents.strip_suffix(' ').ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput(
                    "malformed APT short-break annotation".into(),
                )
            })?;
            if !remainder.is_empty()
                || short_breaks.is_empty()
                || short_breaks
                    .split(' ')
                    .any(|package| !valid_apt_simulation_package(package))
            {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "malformed APT short-break annotation".into(),
                ));
            }
        }
        trailing = remainder;
    }
    Ok(())
}

fn valid_apt_simulation_package(value: &str) -> bool {
    value.split_once(':').map_or_else(
        || safe_apt_token(value),
        |(name, architecture)| {
            safe_apt_token(name) && safe_apt_token(architecture) && !architecture.contains(':')
        },
    )
}

fn valid_apt_release_list(value: &str) -> bool {
    value.split(", ").all(|release| {
        !release.is_empty()
            && release.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '.' | '+' | '-' | '_' | ':' | '/' | '~')
            })
    })
}

fn parse_configured_packages(
    simulation: &str,
) -> Result<BTreeSet<(String, String, String)>, AptResolutionCommandError> {
    let mut configured = BTreeSet::new();
    for line in simulation.lines() {
        if line.split_whitespace().next() == Some("Conf") && line != line.trim() {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT configure action contains leading or trailing whitespace".into(),
            ));
        }
        let rest = match line.strip_prefix("Conf ") {
            Some(rest) => rest,
            None if line.split_whitespace().next() == Some("Conf") => {
                return Err(AptResolutionCommandError::InvalidOutput(
                    "malformed APT configure action".into(),
                ));
            }
            None => continue,
        };
        let (name_with_arch, after_name) = rest.split_once(' ').ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput("malformed APT configure action".into())
        })?;
        let body = after_name
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
            .ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput(
                    "APT configure action must have balanced parentheses".into(),
                )
            })?;
        let (version, after_version) = body.split_once(' ').ok_or_else(|| {
            AptResolutionCommandError::InvalidOutput(
                "APT configure action lacks source and architecture fields".into(),
            )
        })?;
        let (source_list, architecture_field) =
            after_version.rsplit_once(' ').ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput(
                    "APT configure action lacks a source or architecture field".into(),
                )
            })?;
        let listed_architecture = architecture_field
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .ok_or_else(|| {
                AptResolutionCommandError::InvalidOutput(
                    "APT configure action architecture must be bracketed".into(),
                )
            })?;
        let (name, qualified_architecture) = name_with_arch
            .split_once(':')
            .map_or((name_with_arch, None), |(name, arch)| (name, Some(arch)));
        if qualified_architecture.is_some_and(|qualified| qualified != listed_architecture)
            || !safe_apt_token(name)
            || !safe_apt_token(listed_architecture)
            || !safe_apt_version(version)
            || !valid_apt_release_list(source_list)
        {
            return Err(AptResolutionCommandError::InvalidOutput(
                "malformed APT configure action identity".into(),
            ));
        }
        if !configured.insert((
            name.to_owned(),
            listed_architecture.to_owned(),
            version.to_owned(),
        )) {
            return Err(AptResolutionCommandError::InvalidOutput(
                "APT simulation returned a duplicate configure action".into(),
            ));
        }
    }
    Ok(configured)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_safety_evidence_digest_is_stable_for_nonempty_multiarch_state() {
        let live_status_digest = Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap();
        let risks = AptTransactionRisksV1::default();
        let simulation = "Inst curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n\
Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n";
        let installed = vec![
            ("libc6".into(), "arm64".into(), "2.39-0ubuntu8.6".into()),
            ("curl".into(), "amd64".into(), "8.5.0-2ubuntu10.6".into()),
            ("libc6".into(), "amd64".into(), "2.39-0ubuntu8.6".into()),
        ];
        let mut reversed = installed.clone();
        reversed.reverse();

        let digest =
            apt_live_safety_evidence_digest(&live_status_digest, installed, simulation, &risks)
                .unwrap();
        assert_eq!(
            digest,
            apt_live_safety_evidence_digest(&live_status_digest, reversed, simulation, &risks,)
                .unwrap()
        );
        assert_eq!(
            digest.as_str(),
            "sha256:075fde09ddb525b7397d5b2fa680093bcc54b373c88022c89f1db59b309b21a7"
        );
    }

    #[test]
    fn live_safety_evidence_rejects_duplicate_or_malformed_installed_identity() {
        let live_status_digest = Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap();
        let risks = AptTransactionRisksV1::default();
        let valid = ("curl".into(), "amd64".into(), "8.5.0-2ubuntu10.6".into());
        let cases = [
            vec![valid.clone(), valid],
            vec![("curl/bad".into(), "amd64".into(), "1.0".into())],
            vec![("curl".into(), "amd64/bad".into(), "1.0".into())],
            vec![("curl".into(), "amd64".into(), "1.0 bad".into())],
        ];

        for installed in cases {
            assert!(
                apt_live_safety_evidence_digest(&live_status_digest, installed, "", &risks,)
                    .is_err()
            );
        }
    }

    #[test]
    fn empty_live_safety_installed_state_has_an_explicit_v2_digest() {
        let live_status_digest = Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap();

        let digest = apt_live_safety_evidence_digest(
            &live_status_digest,
            Vec::new(),
            "",
            &AptTransactionRisksV1::default(),
        )
        .unwrap();

        // V2 intentionally replaces V1's tuple-keyed JSON object with a closed array.
        assert_eq!(
            digest.as_str(),
            "sha256:b2a66dac8a8a832f4384ad73d49a67f4ea7984595deb19389ce28e1d0388a81b"
        );
    }

    fn fixture_archives(
        print_uris: &str,
    ) -> Result<Vec<AptResolvedArchiveV1>, AptResolutionCommandError> {
        resolve_archives_from_package_metadata(
            "https://archive.ubuntu.com/ubuntu",
            include_str!("../tests/fixtures/apt/simulate-safe.txt"),
            print_uris,
            |package| match package.name.as_str() {
                "curl" => Ok(include_str!("../tests/fixtures/apt/package-show-curl.txt").into()),
                "libc6" => Ok(include_str!("../tests/fixtures/apt/package-show-libc6.txt").into()),
                _ => panic!("unexpected package metadata request"),
            },
        )
    }

    #[test]
    fn parses_sha256_print_uris_fixture_into_exact_archives() {
        let archives =
            fixture_archives(include_str!("../tests/fixtures/apt/print-uris.txt")).unwrap();

        assert_eq!(archives.len(), 2);
        assert_eq!(archives[0].package_name, "curl");
        assert_eq!(archives[0].version, "8.5.0-2ubuntu10.6");
        assert_eq!(archives[0].architecture, "amd64");
        assert_eq!(archives[1].package_name, "libc6");
    }

    #[test]
    fn md5_print_uri_diagnostics_use_signed_package_sha256_metadata() {
        let archives =
            fixture_archives(include_str!("../tests/fixtures/apt/print-uris-md5.txt")).unwrap();

        assert_eq!(archives.len(), 2);
        assert_eq!(
            archives[0].upstream_checksum.as_str(),
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            archives[1].upstream_checksum.as_str(),
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }

    #[test]
    fn signed_package_metadata_rejects_missing_duplicate_or_mismatched_archive_fields() {
        let simulation = "Inst curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n";
        let md5_print_uri = include_str!("../tests/fixtures/apt/print-uris-weak.txt");
        let base = include_str!("../tests/fixtures/apt/package-show-curl.txt");
        let multiple = format!("{base}\n{base}");
        let cases = [
            base.replace("Version: 8.5.0-2ubuntu10.6", "Version: 8.5.0-unsafe"),
            base.replace("Architecture: amd64", "Architecture: arm64"),
            base.replace("Size: 14", "Size: 15"),
            base.replace(
                "curl_8.5.0-2ubuntu10.6_amd64.deb",
                "curl_8.5.0-unsafe_amd64.deb",
            ),
            base.replace(
                "SHA256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
                "",
            ),
            format!("Package: other\n{base}"),
            multiple,
        ];

        for metadata in cases {
            assert!(
                resolve_archives_from_package_metadata(
                    "https://archive.ubuntu.com/ubuntu",
                    simulation,
                    md5_print_uri,
                    |_| Ok(metadata.clone()),
                )
                .is_err()
            );
        }

        let sha256_mismatch = include_str!("../tests/fixtures/apt/print-uris.txt")
            .lines()
            .next()
            .unwrap()
            .replace(
                "SHA256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "SHA256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            );
        assert!(
            resolve_archives_from_package_metadata(
                "https://archive.ubuntu.com/ubuntu",
                simulation,
                &sha256_mismatch,
                |_| Ok(base.into()),
            )
            .is_err()
        );
    }

    #[test]
    fn archive_uri_must_match_the_full_signed_filename_under_the_canonical_repository() {
        let simulation = "Inst curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n";
        let metadata = include_str!("../tests/fixtures/apt/package-show-curl.txt");
        let filename = "curl_8.5.0-2ubuntu10.6_amd64.deb";
        let cases = [
            format!(
                "'https://archive.ubuntu.com/ubuntu/pool/universe/c/curl/{filename}' {filename} 14 MD5Sum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ),
            format!(
                "'https://mirror.invalid/ubuntu/pool/main/c/curl/{filename}' {filename} 14 MD5Sum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ),
            format!(
                "'http://archive.ubuntu.com/ubuntu/pool/main/c/curl/{filename}' {filename} 14 MD5Sum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ),
            format!(
                "'https://archive.ubuntu.com/ubuntu/pool/main/c/curl/../curl/{filename}' {filename} 14 MD5Sum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ),
            format!(
                "'https://archive.ubuntu.com/ubuntu/pool/main/c/%63url/{filename}' {filename} 14 MD5Sum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ),
        ];

        for print_uri in cases {
            assert!(
                resolve_archives_from_package_metadata(
                    "https://archive.ubuntu.com/ubuntu",
                    simulation,
                    &print_uri,
                    |_| Ok(metadata.into()),
                )
                .is_err(),
                "accepted untrusted archive URI: {print_uri}"
            );
        }
    }

    #[test]
    fn print_uri_output_rejects_every_unknown_nonempty_line() {
        let valid = include_str!("../tests/fixtures/apt/print-uris-weak.txt");
        for hostile in [
            format!("unexpected APT output\n{valid}"),
            valid.replacen('\'', "", 1),
            format!(
                "{valid}https://mirror.invalid/curl.deb curl.deb 14 MD5Sum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ),
        ] {
            assert!(
                parse_print_uri_records(&hostile).is_err(),
                "accepted unknown print-URI output: {hostile}"
            );
        }
    }

    #[test]
    fn package_metadata_command_is_fixed_private_and_exact() {
        let commands = ProcessAptResolutionCommandRunner::package_metadata_command_snapshot(&[
            AptResolvedPackageV1 {
                name: "curl".into(),
                version: "8.5.0-2ubuntu10.6".into(),
                architecture: "amd64".into(),
            },
        ])
        .unwrap();
        let command = &commands[0];
        let rendered = command.args.join(" ");

        assert_eq!(command.executable, "/usr/bin/apt-cache");
        assert!(!command.network);
        assert_eq!(
            command.environment.get("APT_CONFIG").map(String::as_str),
            Some("<private>/apt.conf")
        );
        assert!(rendered.contains("Dir::State::lists=<private>/lists"));
        assert!(rendered.contains("Dir::State::status=<private>/status"));
        assert!(rendered.contains("Dir::Cache::pkgcache=<private>/pkgcache.bin"));
        assert!(rendered.contains("Dir::Cache::srcpkgcache=<private>/srcpkgcache.bin"));
        assert!(rendered.ends_with("show curl:amd64=8.5.0-2ubuntu10.6"));
    }

    #[test]
    fn manager_binding_includes_the_apt_cache_executable() {
        let digest = |character: char| {
            Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
        };
        let binding = |apt_cache_digest: &Sha256Digest| {
            manager_binding_from_evidence(
                "apt 2.7.14 (amd64)\n",
                "Debian 'dpkg' package management program version 1.22.6 (amd64).\n",
                "Acquire::Languages \"none\";\n",
                "deb [arch=amd64 signed-by=<private>/archive-keyring.gpg] https://archive.ubuntu.com/ubuntu noble main\n",
                "amd64",
                "ID=ubuntu\nVERSION_ID=24.04\nVERSION_CODENAME=noble\n",
                &digest('a'),
                apt_cache_digest,
                &digest('b'),
                &digest('c'),
                &digest('d'),
            )
            .unwrap()
        };

        assert_ne!(binding(&digest('e')), binding(&digest('f')));
    }

    #[test]
    fn real_solver_metadata_marks_removal_replacement_and_alternatives_unsafe() {
        let risks = parse_solver_risks(
            include_str!("../tests/fixtures/apt/simulate-malicious.txt"),
            include_str!("../tests/fixtures/apt/depends-malicious.txt"),
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
            BTreeSet::from(["libssl3 | libssl1.1".into()])
        );
    }

    #[test]
    fn private_apt_configuration_rejects_host_hook_and_helper_overrides() {
        let config = private_apt_config("/private");
        assert!(config.contains("Dir::Etc::main \"/private/empty-main\""));
        assert!(config.contains("Dir::Etc::parts \"/private/empty-parts\""));
        assert!(validate_effective_apt_config("Acquire::Languages \"none\";").is_ok());
        for hostile in [
            "DPkg::Pre-Invoke:: \"/host/hook\";",
            "Acquire::http::Proxy-Auto-Detect \"/host/proxy\";",
            "Dir::Bin::solvers \"/host/solver\";",
            "Dir::Bin::methods \"/host/methods\";",
            "APT::Solver \"/host/solver\";",
        ] {
            assert!(validate_effective_apt_config(hostile).is_err(), "{hostile}");
        }
    }

    #[test]
    fn root_apt_configuration_requires_the_bound_sandbox_identity() {
        validate_root_apt_sandbox_config("APT::Sandbox::User \"_apt\";").unwrap();
        assert!(validate_root_apt_sandbox_config("Acquire::Languages \"none\";").is_err());
        assert!(validate_root_apt_sandbox_config("APT::Sandbox::User \"root\";").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn signing_key_snapshot_is_no_follow_owner_bound_and_non_writable() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let root = tempfile::tempdir().unwrap();
        let key = root.path().join("archive.gpg");
        fs::write(&key, b"trusted key bytes").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
        let owner = fs::metadata(&key).unwrap().uid();
        assert_eq!(
            read_trusted_key_for_uid(&key, owner).unwrap(),
            b"trusted key bytes"
        );

        fs::set_permissions(&key, fs::Permissions::from_mode(0o664)).unwrap();
        assert!(read_trusted_key_for_uid(&key, owner).is_err());
        fs::remove_file(&key).unwrap();
        std::os::unix::fs::symlink(root.path().join("elsewhere"), &key).unwrap();
        assert!(read_trusted_key_for_uid(&key, owner).is_err());
    }

    #[test]
    fn simulation_and_archive_records_must_name_the_same_exact_closure() {
        let archives =
            fixture_archives(include_str!("../tests/fixtures/apt/print-uris.txt")).unwrap();
        validate_simulated_archive_closure(
            include_str!("../tests/fixtures/apt/simulate-safe.txt"),
            &archives,
        )
        .unwrap();
        assert!(
            validate_simulated_archive_closure(
                include_str!("../tests/fixtures/apt/simulate-malicious.txt"),
                &archives,
            )
            .is_err()
        );
    }

    #[test]
    fn parses_real_inst_line_with_a_trailing_empty_break_annotation() {
        assert_eq!(
            parse_simulated_packages(include_str!(
                "../tests/fixtures/apt/simulate-inst-annotations.txt"
            ))
            .unwrap(),
            BTreeSet::from([(
                "libgcc-s1".into(),
                "amd64".into(),
                "14-20240412-0ubuntu1".into(),
            )])
        );
    }

    #[test]
    fn parses_complete_inst_upgrade_and_dependency_annotation_grammar() {
        let line = concat!(
            "Inst curl:amd64 [8.5.0-2ubuntu10.5] ",
            "(8.5.0-2ubuntu10.6 Ubuntu:24.04/noble, Ubuntu:24.04/noble-updates [amd64]) ",
            "[libc6:amd64 on libssl3:amd64] [broken:amd64 other ]\n",
        );

        assert_eq!(
            parse_simulated_packages(line).unwrap(),
            BTreeSet::from([("curl".into(), "amd64".into(), "8.5.0-2ubuntu10.6".into(),)])
        );
    }

    #[test]
    fn inst_parser_rejects_malformed_groups_junk_and_architecture_conflicts() {
        let valid_primary = "(14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64])";
        let malformed = [
            "Inst",
            "Inst\tlibgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64])",
            " Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64])",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64]",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64]))",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64]) junk",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64]) [",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64]) [pkg on]",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64]) [] [later:amd64 on dependency:amd64]",
            "Inst libgcc-s1:arm64 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64])",
            "Inst libgcc-s1 (14-20240412-0ubuntu1 Ubuntu:24.04/noble [])",
            "Inst libgcc-s1 [] (14-20240412-0ubuntu1 Ubuntu:24.04/noble [amd64])",
        ];
        for line in malformed {
            assert!(
                parse_simulated_packages(&format!("{line}\n")).is_err(),
                "accepted malformed Inst line: {line}"
            );
        }

        assert_eq!(
            parse_simulated_packages(&format!("Inst libgcc-s1 {valid_primary} [arm64 ]\n"))
                .unwrap(),
            BTreeSet::from([(
                "libgcc-s1".into(),
                "amd64".into(),
                "14-20240412-0ubuntu1".into(),
            )])
        );
    }

    #[test]
    fn installed_state_requires_complete_consistent_dpkg_observation() {
        let status = concat!(
            "Package: curl\n",
            "Status: install ok installed\n",
            "Architecture: amd64\n",
            "Version: 8.5.0-2ubuntu10.6\n\n",
            "Package: old-lib\n",
            "Status: deinstall ok config-files\n",
            "Architecture: amd64\n",
            "Version: 1.0-1\n",
        );
        let query = "curl:amd64\tii \t8.5.0-2ubuntu10.6\tamd64\n";

        let observed = parse_complete_installed_state(status, query).unwrap();
        assert_eq!(
            observed
                .get(&("curl".to_owned(), "amd64".to_owned()))
                .map(String::as_str),
            Some("8.5.0-2ubuntu10.6")
        );
        assert!(parse_complete_installed_state(status, "").is_err());
        assert!(parse_complete_installed_state(status, "curl\tii \t8.5.0\n").is_err());
        assert!(parse_complete_installed_state(status, &format!("{query}{query}")).is_err());
        assert!(
            parse_complete_installed_state(status, &format!("{query}ghost\t???\t1.0-1\tamd64\n"),)
                .is_err()
        );
        assert!(
            parse_complete_installed_state(
                &status.replace("install ok installed", "banana ok installed"),
                query,
            )
            .is_err()
        );
        let broken = format!(
            "{status}\nPackage: broken\nStatus: install reinstreq installed\nArchitecture: amd64\nVersion: 1.0-1\n"
        );
        assert!(parse_complete_installed_state(&broken, query).is_err());
    }

    #[test]
    fn nonterminal_dpkg_state_stops_before_network_continuation() {
        let query = "base-files:amd64\tii \t1.0-1\tamd64\n";
        for package_status in [
            "install ok unpacked",
            "install ok half-configured",
            "install ok triggers-awaited",
            "install ok triggers-pending",
            "install reinstreq installed",
        ] {
            let status = format!(
                "Package: base-files\nStatus: install ok installed\nArchitecture: amd64\nVersion: 1.0-1\n\nPackage: broken\nStatus: {package_status}\nArchitecture: amd64\nVersion: 2.0-1\n"
            );
            let mut network_calls = 0;

            assert!(
                with_complete_installed_state(&status, query, |_| {
                    network_calls += 1;
                    Ok(())
                })
                .is_err(),
                "{package_status}"
            );
            assert_eq!(network_calls, 0, "{package_status}");
        }
    }

    #[test]
    fn live_safety_simulation_rejects_reverse_conflict_side_effects() {
        let closure = vec![AptResolvedPackageV1 {
            name: "curl".into(),
            version: "8.5.0-2ubuntu10.6".into(),
            architecture: "amd64".into(),
        }];

        validate_live_safety_simulation("", &closure).unwrap();
        assert!(validate_live_safety_simulation("Remv old-lib [1.0-1]\n", &closure).is_err());
        assert!(
            validate_live_safety_simulation(
                "Inst unrelated (2.0-1 Ubuntu:24.04/noble [amd64])\n",
                &closure,
            )
            .is_err()
        );
    }

    #[test]
    fn live_safety_simulation_binds_every_conf_action_to_the_pinned_closure() {
        let closure = vec![AptResolvedPackageV1 {
            name: "curl".into(),
            version: "8.5.0-2ubuntu10.6".into(),
            architecture: "amd64".into(),
        }];
        let exact = "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n";

        validate_live_safety_simulation(exact, &closure).unwrap();
        for unsafe_output in [
            "Conf unrelated (1.0-1 Ubuntu:24.04/noble [amd64])\n",
            "Conf curl (9.0-1 Ubuntu:24.04/noble [amd64])\n",
            &format!("{exact}{exact}"),
            "Conf curl malformed\n",
            "Conf\n",
        ] {
            assert!(
                validate_live_safety_simulation(unsafe_output, &closure).is_err(),
                "{unsafe_output}"
            );
        }
    }

    #[test]
    fn live_safety_simulation_accepts_a_real_multi_origin_conf_action() {
        let closure = vec![AptResolvedPackageV1 {
            name: "curl".into(),
            version: "8.5.0-2ubuntu10.6".into(),
            architecture: "amd64".into(),
        }];
        let simulation = "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64])\n";

        validate_live_safety_simulation(simulation, &closure).unwrap();
    }

    #[test]
    fn live_safety_simulation_rejects_malformed_multi_origin_conf_actions() {
        let closure = vec![AptResolvedPackageV1 {
            name: "curl".into(),
            version: "8.5.0-2ubuntu10.6".into(),
            architecture: "amd64".into(),
        }];
        let prefix = "Conf curl (8.5.0-2ubuntu10.6 ";
        let suffix = " [amd64])\n";

        for malformed_sources in [
            "Ubuntu:24.04/noble-updates,Ubuntu:24.04/noble-security",
            "Ubuntu:24.04/noble-updates,  Ubuntu:24.04/noble-security",
            "Ubuntu:24.04/noble-updates , Ubuntu:24.04/noble-security",
            "Ubuntu:24.04/noble-updates,, Ubuntu:24.04/noble-security",
            "Ubuntu:24.04/noble-updates, , Ubuntu:24.04/noble-security",
            ", Ubuntu:24.04/noble-security",
            "Ubuntu:24.04/noble-updates,",
        ] {
            let simulation = format!("{prefix}{malformed_sources}{suffix}");
            assert!(
                validate_live_safety_simulation(&simulation, &closure).is_err(),
                "{simulation}"
            );
        }

        for malformed_line in [
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64]\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security amd64)\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [amd64]) junk\n",
        ] {
            assert!(
                validate_live_safety_simulation(malformed_line, &closure).is_err(),
                "{malformed_line}"
            );
        }
    }

    #[test]
    fn live_safety_simulation_rejects_malformed_conf_grammar() {
        let closure = vec![AptResolvedPackageV1 {
            name: "curl".into(),
            version: "8.5.0-2ubuntu10.6".into(),
            architecture: "amd64".into(),
        }];

        for malformed in [
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64]\n",
            "Conf curl 8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n",
            "Conf curl:amd64 (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble\n",
            "Conf curl (8.5.0-2ubuntu10.6 [amd64])\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble extra [amd64])\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64] extra)\n",
            "Conf curl:amd64 (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64]) trailing\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble amd64)\n",
            "Conf curl ((8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64]))\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [[amd64]])\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64]])\n",
            " Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64])\n",
            "Conf curl (8.5.0-2ubuntu10.6 Ubuntu:24.04/noble [amd64]) \n",
        ] {
            assert!(
                validate_live_safety_simulation(malformed, &closure).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn apt_diagnostics_reject_unsandboxed_root_fallback() {
        validate_apt_command_diagnostics("").unwrap();
        assert!(
            validate_apt_command_diagnostics(
                "W: Download is performed unsandboxed as root because user '_apt' cannot access the file",
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn dpkg_observation_requires_a_zero_exit_status() {
        let workspace = tempfile::tempdir().unwrap();
        let command = AptCommandSpecV1 {
            executable: "/usr/bin/false".into(),
            args: Vec::new(),
            environment: BTreeMap::new(),
            network: false,
        };

        assert!(matches!(
            run_fixed_command(&command, workspace.path().to_str().unwrap()),
            Err(AptResolutionCommandError::Failed(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn private_workspace_grants_only_required_apt_sandbox_access() {
        use std::os::unix::fs::MetadataExt;

        let root = tempfile::tempdir().unwrap();
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let identity = AptSandboxIdentityV1 { uid, gid };
        write_private_apt_workspace_for_identity(
            root.path(),
            "deb [signed-by=/private/key] https://example.invalid noble main\n",
            b"key",
            b"Package: base-files\nStatus: install ok installed\nArchitecture: amd64\nVersion: 1.0-1\n",
            Some(identity),
        )
        .unwrap();

        assert_eq!(fs::metadata(root.path()).unwrap().mode() & 0o777, 0o750);
        assert_eq!(
            fs::metadata(root.path().join("archive-keyring.gpg"))
                .unwrap()
                .mode()
                & 0o777,
            0o440
        );
        for partial in ["lists/partial", "archives/partial"] {
            let metadata = fs::metadata(root.path().join(partial)).unwrap();
            assert_eq!(metadata.uid(), uid);
            assert_eq!(metadata.gid(), gid);
            assert_eq!(metadata.mode() & 0o777, 0o700);
        }
        assert!(select_apt_sandbox_identity(0, "root:x:0:0:root:/root:/bin/sh\n").is_err());
        assert_eq!(
            select_apt_sandbox_identity(
                0,
                "root:x:0:0:root:/root:/bin/sh\n_apt:x:42:65534::/nonexistent:/usr/sbin/nologin\n",
            )
            .unwrap(),
            Some(AptSandboxIdentityV1 {
                uid: 42,
                gid: 65_534,
            })
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

    #[cfg(unix)]
    #[test]
    fn apt_executable_digest_does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("apt-cache");
        let linked = root.path().join("linked-apt-cache");
        fs::write(&executable, b"trusted apt-cache bytes").unwrap();
        std::os::unix::fs::symlink(&executable, &linked).unwrap();

        assert_eq!(
            digest_no_follow_regular_file(&executable).unwrap(),
            content_digest(b"trusted apt-cache bytes").unwrap()
        );
        assert!(matches!(
            digest_no_follow_regular_file(&linked),
            Err(AptResolutionCommandError::Unavailable(_))
        ));
    }
}
