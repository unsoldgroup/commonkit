use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use commonkit_contracts::{
    PackageManager, PackageSelector, Sha256Digest, StableId, digest_domain_json,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    ArtifactEvidence, ManagerBindingV1, NodeOfflineInstallRecipeV1, OfflineInstallRecipeV1,
    PackageDiscoveryFetchRequestV1, PackageFetch, PackageFetchHopV1, PackageFetchRequestV1,
    PackageFetchedResolutionV1, PackageObservationV1, PackageResolutionBackend,
    PackageResolutionDraftV1, PackageResolutionError, PackageResolutionProbeV1,
    PackageResolutionRequestV1, PackageTargetV1, ResolvedPackage, SourceBindingV1,
};

const MAX_RELEASE_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_NODE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRuntimeHostSnapshotV1 {
    pub target: PackageTargetV1,
    pub manager: ManagerBindingV1,
    pub before: PackageObservationV1,
    pub nvm_script_digest: Sha256Digest,
    pub shell_executable_digest: Sha256Digest,
    pub release_keyring_digest: Sha256Digest,
    pub release_keyring: Vec<u8>,
}

pub trait NodeRuntimeHost {
    fn probe(
        &mut self,
        target: &PackageTargetV1,
    ) -> Result<NodeRuntimeHostSnapshotV1, NodeRuntimeHostError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedNodeReleaseSignatureV1 {
    pub signer_fingerprint: String,
    pub signed_payload: Vec<u8>,
}

pub trait NodeReleaseSignatureVerifier {
    fn verify(
        &mut self,
        armored_signature: &[u8],
        keyring: &[u8],
    ) -> Result<VerifiedNodeReleaseSignatureV1, NodeReleaseSignatureError>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NodeRuntimeHostError {
    #[error("required nvm host capability is unavailable: {0}")]
    Unavailable(String),
    #[error("nvm host configuration is unsafe: {0}")]
    UnsafeConfiguration(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NodeReleaseSignatureError {
    #[error("Node release signature verification is unavailable: {0}")]
    Unavailable(String),
    #[error("Node release signature is invalid: {0}")]
    Invalid(String),
}

pub struct NodeResolutionBackend<'a, H, V> {
    host: &'a mut H,
    verifier: &'a mut V,
    prepared_host: Option<NodeRuntimeHostSnapshotV1>,
}

impl<'a, H, V> NodeResolutionBackend<'a, H, V> {
    pub fn new(host: &'a mut H, verifier: &'a mut V) -> Self {
        Self {
            host,
            verifier,
            prepared_host: None,
        }
    }
}

impl<H, V> PackageResolutionBackend for NodeResolutionBackend<'_, H, V>
where
    H: NodeRuntimeHost,
    V: NodeReleaseSignatureVerifier,
{
    fn manager(&self) -> PackageManager {
        PackageManager::Nvm
    }

    fn preflight_resolution(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
    ) -> Result<(), PackageResolutionError> {
        validate_node_request(request)?;
        node_platform(request.target)?;
        if self.prepared_host.is_none() {
            let host = self.host.probe(request.target).map_err(|error| {
                PackageResolutionError::NodeBackend {
                    reason: error.to_string(),
                }
            })?;
            validate_host_snapshot(request, &host)?;
            self.prepared_host = Some(host);
        } else if let Some(host) = &self.prepared_host {
            validate_host_snapshot(request, host)?;
        }
        Ok(())
    }

    fn probe(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
    ) -> Result<PackageResolutionProbeV1, PackageResolutionError> {
        Err(PackageResolutionError::InvalidNodeRequest)
    }

    fn resolve(
        &mut self,
        _request: &PackageResolutionRequestV1<'_>,
        _source: &SourceBindingV1,
    ) -> Result<PackageResolutionDraftV1, PackageResolutionError> {
        Err(PackageResolutionError::InvalidNodeRequest)
    }

    fn resolve_and_fetch(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
        fetch: &mut dyn PackageFetch,
    ) -> Result<Option<PackageFetchedResolutionV1>, PackageResolutionError> {
        self.resolve_node(request, fetch).map(Some)
    }
}

impl<H, V> NodeResolutionBackend<'_, H, V>
where
    H: NodeRuntimeHost,
    V: NodeReleaseSignatureVerifier,
{
    fn resolve_node(
        &mut self,
        request: &PackageResolutionRequestV1<'_>,
        fetch: &mut dyn PackageFetch,
    ) -> Result<PackageFetchedResolutionV1, PackageResolutionError> {
        validate_node_request(request)?;
        let declaration = match request.desired {
            crate::PackageDesiredIntent::Package { declaration } => declaration,
        };
        let authority = request
            .node_source_authority
            .ok_or(PackageResolutionError::InvalidNodeRequest)?;
        let host = self
            .prepared_host
            .clone()
            .ok_or(PackageResolutionError::NodeAuthorityMismatch)?;
        validate_host_snapshot(request, &host)?;

        let version = declaration
            .version
            .strip_prefix('v')
            .unwrap_or(&declaration.version);
        let platform = node_platform(request.target)?;
        let archive_name = format!("node-v{version}-{platform}.tar.xz");
        let version_root = format!("{}/v{version}", request.canonical_repository);
        let sums_locator = format!("{version_root}/SHASUMS256.txt");
        let signature_locator = format!("{version_root}/SHASUMS256.txt.asc");
        let archive_locator = format!("{version_root}/{archive_name}");
        let locators = vec![
            sums_locator.clone(),
            signature_locator.clone(),
            archive_locator.clone(),
        ];
        fetch.preflight_locators(&locators)?;

        let sums = fetch_discovery(
            fetch,
            "node-checksums",
            &sums_locator,
            MAX_RELEASE_METADATA_BYTES,
        )?;
        let signature = fetch_discovery(
            fetch,
            "node-signature",
            &signature_locator,
            MAX_RELEASE_METADATA_BYTES,
        )?;
        let verified = self
            .verifier
            .verify(&signature, &host.release_keyring)
            .map_err(|error| PackageResolutionError::NodeBackend {
                reason: error.to_string(),
            })?;
        let fingerprint = verified.signer_fingerprint.to_ascii_lowercase();
        if verified.signed_payload != sums
            || !authority.release_key_fingerprints.contains(&fingerprint)
        {
            return Err(PackageResolutionError::UnauthenticatedNodeMetadata);
        }
        let archive_checksum = checksum_for_archive(&sums, &archive_name)?;
        let archive = fetch_discovery(
            fetch,
            "node-archive",
            &archive_locator,
            MAX_NODE_ARCHIVE_BYTES,
        )?;
        if content_digest(&archive)? != archive_checksum {
            return Err(PackageResolutionError::CorruptArtifact);
        }

        let sums_digest = content_digest(&sums)?;
        let signature_digest = content_digest(&signature)?;
        let evidence = ArtifactEvidence {
            authority: StableId::parse("node-release-key")?,
            metadata_digest: sums_digest.clone(),
            signature_digest: signature_digest.clone(),
        };
        let probe = PackageResolutionProbeV1 {
            before: host.before.clone(),
            repository_revision: Some(sums_digest.as_str().trim_start_matches("sha256:").into()),
            signed_metadata: vec![evidence],
        };
        let source = SourceBindingV1 {
            source_id: request.source_id.clone(),
            registry_definition_digest: request.registry_definition_digest.clone(),
            canonical_repository: request.canonical_repository.into(),
            repository_revision: probe.repository_revision.clone(),
            signed_metadata: probe.signed_metadata.clone(),
        };
        let source_metadata_digest = source.metadata_digest()?;
        let artifacts = vec![
            artifact_request(
                "node-archive",
                &archive_locator,
                &archive,
                &format!(
                    "node-v{}-{}",
                    version.replace('.', "-"),
                    platform.replace('_', "-")
                ),
                &source_metadata_digest,
            )?,
            artifact_request(
                "node-checksums",
                &sums_locator,
                &sums,
                &format!("node-v{}-checksums", version.replace('.', "-")),
                &source_metadata_digest,
            )?,
            artifact_request(
                "node-signature",
                &signature_locator,
                &signature,
                &format!("node-v{}-signature", version.replace('.', "-")),
                &source_metadata_digest,
            )?,
        ];
        let fetched = vec![
            (artifacts[0].clone(), archive),
            (artifacts[1].clone(), sums),
            (artifacts[2].clone(), signature),
        ];
        let artifact_roles = artifacts
            .iter()
            .map(|artifact| artifact.role.clone())
            .collect();
        let cache_relative_path = format!(".cache/bin/node-v{version}-{platform}/{archive_name}");
        let draft = PackageResolutionDraftV1 {
            closure: vec![ResolvedPackage {
                declaration: declaration.clone(),
                source,
            }],
            artifacts,
            recipe: OfflineInstallRecipeV1::NodeArchive {
                artifact_roles,
                install: Some(NodeOfflineInstallRecipeV1 {
                    node_version: version.into(),
                    archive_file_name: archive_name,
                    cache_relative_path,
                    nvm_version: host.manager.version.clone(),
                    nvm_script_digest: host.nvm_script_digest,
                    shell_executable_digest: host.shell_executable_digest,
                    offline: true,
                    no_source_fallback: true,
                    per_version_lock: true,
                    install_latest_npm: false,
                    migrate_packages: false,
                }),
            },
        };
        Ok(PackageFetchedResolutionV1 {
            probe,
            draft,
            fetched,
        })
    }
}

fn validate_node_request(
    request: &PackageResolutionRequestV1<'_>,
) -> Result<(), PackageResolutionError> {
    let declaration = match request.desired {
        crate::PackageDesiredIntent::Package { declaration } => declaration,
    };
    if declaration.manager != PackageManager::Nvm
        || request.manager.manager != PackageManager::Nvm
        || !matches!(declaration.selector, Some(PackageSelector::NodeRuntime {}))
        || declaration.source != *request.source_id
        || request.canonical_repository != "https://nodejs.org/dist"
        || request.node_source_authority.is_none()
    {
        Err(PackageResolutionError::InvalidNodeRequest)
    } else {
        Ok(())
    }
}

fn fetch_discovery(
    fetch: &mut dyn PackageFetch,
    role: &str,
    locator: &str,
    maximum_bytes: u64,
) -> Result<Vec<u8>, PackageResolutionError> {
    let request = PackageDiscoveryFetchRequestV1 {
        role: StableId::parse(role)?,
        immutable_locator: locator.into(),
        maximum_bytes,
    };
    match fetch.fetch_discovery_hop(&request, locator)? {
        PackageFetchHopV1::Complete(result) => Ok(result.bytes),
        PackageFetchHopV1::Redirect { .. } => Err(PackageResolutionError::UnvalidatedRedirect),
    }
}

fn artifact_request(
    role: &str,
    locator: &str,
    bytes: &[u8],
    materialization_key: &str,
    source_metadata_digest: &Sha256Digest,
) -> Result<PackageFetchRequestV1, PackageResolutionError> {
    Ok(PackageFetchRequestV1 {
        role: StableId::parse(role)?,
        immutable_locator: locator.into(),
        upstream_checksum: content_digest(bytes)?,
        size: u64::try_from(bytes.len()).map_err(|_| PackageResolutionError::CorruptArtifact)?,
        materialization_key: StableId::parse(materialization_key)?,
        source_metadata_digest: source_metadata_digest.clone(),
    })
}

fn checksum_for_archive(
    checksums: &[u8],
    expected_name: &str,
) -> Result<Sha256Digest, PackageResolutionError> {
    let text = std::str::from_utf8(checksums)
        .map_err(|_| PackageResolutionError::UnauthenticatedNodeMetadata)?;
    let mut matched = None;
    for line in text.lines() {
        let (checksum, name) = line
            .split_once("  ")
            .ok_or(PackageResolutionError::UnauthenticatedNodeMetadata)?;
        if checksum.len() != 64
            || !checksum
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
            || !safe_release_file_name(name)
        {
            return Err(PackageResolutionError::UnauthenticatedNodeMetadata);
        }
        if name == expected_name {
            if matched.is_some() {
                return Err(PackageResolutionError::UnauthenticatedNodeMetadata);
            }
            matched = Some(Sha256Digest::parse(format!("sha256:{checksum}"))?);
        }
    }
    matched.ok_or(PackageResolutionError::IncompleteNodeRelease)
}

fn safe_release_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && name.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && part.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
                })
        })
}

pub(crate) fn node_platform(
    target: &PackageTargetV1,
) -> Result<&'static str, PackageResolutionError> {
    match (
        target.os.as_str(),
        target.arch.as_str(),
        target.libc.as_deref(),
    ) {
        ("linux", "amd64" | "x86_64", Some("glibc")) => Ok("linux-x64"),
        ("linux", "arm64" | "aarch64", Some("glibc")) => Ok("linux-arm64"),
        ("darwin" | "macos", "amd64" | "x86_64", _) => Ok("darwin-x64"),
        ("darwin" | "macos", "arm64" | "aarch64", _) => Ok("darwin-arm64"),
        _ => Err(PackageResolutionError::UnsupportedNodeTarget),
    }
}

fn validate_host_snapshot(
    request: &PackageResolutionRequestV1<'_>,
    host: &NodeRuntimeHostSnapshotV1,
) -> Result<(), PackageResolutionError> {
    if &host.target != request.target
        || &host.manager != request.manager
        || host.manager.manager != PackageManager::Nvm
        || !nvm_at_least_0_40_6(&host.manager.version)
        || host.manager.executable_digest != host.nvm_script_digest
        || content_digest(&host.release_keyring)? != host.release_keyring_digest
    {
        return Err(PackageResolutionError::NodeAuthorityMismatch);
    }
    let prefix = request
        .target
        .manager_prefix
        .as_deref()
        .ok_or(PackageResolutionError::InvalidNodeRequest)?;
    let expected_config_digest = digest_domain_json(
        "commonkit.nvm-manager-config.v1",
        &(
            &host.shell_executable_digest,
            &host.release_keyring_digest,
            prefix,
        ),
    )?;
    if host.manager.config_digest != expected_config_digest {
        return Err(PackageResolutionError::NodeAuthorityMismatch);
    }
    Ok(())
}

fn content_digest(bytes: &[u8]) -> Result<Sha256Digest, PackageResolutionError> {
    Ok(Sha256Digest::parse(format!(
        "sha256:{:x}",
        Sha256::digest(bytes)
    ))?)
}

#[derive(Debug, Clone)]
pub struct ProcessNodeRuntimeHost {
    nvm_dir: PathBuf,
    shell_executable: PathBuf,
    release_keyring: PathBuf,
}

impl ProcessNodeRuntimeHost {
    pub fn new(
        nvm_dir: impl Into<PathBuf>,
        shell_executable: impl Into<PathBuf>,
        release_keyring: impl Into<PathBuf>,
    ) -> Self {
        Self {
            nvm_dir: nvm_dir.into(),
            shell_executable: shell_executable.into(),
            release_keyring: release_keyring.into(),
        }
    }
}

impl NodeRuntimeHost for ProcessNodeRuntimeHost {
    fn probe(
        &mut self,
        target: &PackageTargetV1,
    ) -> Result<NodeRuntimeHostSnapshotV1, NodeRuntimeHostError> {
        let prefix = target.manager_prefix.as_deref().ok_or_else(|| {
            NodeRuntimeHostError::UnsafeConfiguration("NVM_DIR is not bound".into())
        })?;
        if Path::new(prefix) != self.nvm_dir {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(
                "NVM_DIR differs from the bound manager prefix".into(),
            ));
        }
        let nvm_script = read_regular_no_follow(&self.nvm_dir.join("nvm.sh"))?;
        let nvm_version = parse_nvm_version(&nvm_script)?;
        if !nvm_at_least_0_40_6(&nvm_version) {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(
                "nvm 0.40.6 or newer is required".into(),
            ));
        }
        match fs::symlink_metadata(self.nvm_dir.join("default-packages")) {
            Ok(_) => {
                return Err(NodeRuntimeHostError::UnsafeConfiguration(
                    "nvm default-packages is not allowed".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(NodeRuntimeHostError::Unavailable(error.to_string())),
        }
        if self.nvm_dir.file_name().and_then(|name| name.to_str()) != Some(".nvm") {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(
                "production nvm resolution requires the bound user .nvm directory".into(),
            ));
        }
        let npmrc = self
            .nvm_dir
            .parent()
            .ok_or_else(|| {
                NodeRuntimeHostError::UnsafeConfiguration("NVM_DIR has no user root".into())
            })?
            .join(".npmrc");
        if let Some(bytes) = read_optional_regular_no_follow(&npmrc)? {
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                NodeRuntimeHostError::UnsafeConfiguration(".npmrc is not UTF-8".into())
            })?;
            if text.lines().any(|line| {
                line.split_once('=')
                    .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("prefix"))
            }) {
                return Err(NodeRuntimeHostError::UnsafeConfiguration(
                    ".npmrc prefix is not allowed".into(),
                ));
            }
        }
        validate_nvm_environment_os(&std::env::vars_os().collect())?;
        let shell = read_executable_no_follow(&self.shell_executable)?;
        let keyring = read_regular_no_follow(&self.release_keyring)?;
        let nvm_script_digest = host_content_digest(&nvm_script)?;
        let shell_executable_digest = host_content_digest(&shell)?;
        let release_keyring_digest = host_content_digest(&keyring)?;
        let config_digest = digest_domain_json(
            "commonkit.nvm-manager-config.v1",
            &(&shell_executable_digest, &release_keyring_digest, prefix),
        )
        .map_err(|error| NodeRuntimeHostError::Unavailable(error.to_string()))?;
        let before = observe_installed_versions(&self.nvm_dir)?;
        Ok(NodeRuntimeHostSnapshotV1 {
            target: target.clone(),
            manager: ManagerBindingV1 {
                manager: PackageManager::Nvm,
                version: nvm_version,
                executable_digest: nvm_script_digest.clone(),
                config_digest,
            },
            before,
            nvm_script_digest,
            shell_executable_digest,
            release_keyring_digest,
            release_keyring: keyring,
        })
    }
}

pub fn validate_nvm_environment(
    environment: &BTreeMap<String, String>,
) -> Result<(), NodeRuntimeHostError> {
    validate_nvm_environment_os(
        &environment
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
    )
}

pub fn validate_nvm_environment_os(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<(), NodeRuntimeHostError> {
    for (key, value) in environment {
        let variable = key.to_str().ok_or_else(|| {
            NodeRuntimeHostError::UnsafeConfiguration(
                "non-UTF-8 environment variable name is not allowed".into(),
            )
        })?;
        if value.to_str().is_none() {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(format!(
                "non-UTF-8 value for {variable} is not allowed"
            )));
        }
        let canonical = variable.to_ascii_uppercase();
        if canonical.starts_with("NPM_CONFIG_")
            || matches!(
                canonical.as_str(),
                "PREFIX"
                    | "NVM_NODEJS_ORG_MIRROR"
                    | "NVM_IOJS_ORG_MIRROR"
                    | "NVM_REINSTALL_PACKAGES_FROM"
                    | "NVM_INSTALL_LATEST_NPM"
            )
        {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(format!(
                "{variable} is not allowed"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ProcessNodeReleaseSignatureVerifier {
    gpgv_executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSignatureCommandSpecV1 {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

impl ProcessNodeReleaseSignatureVerifier {
    pub fn new(gpgv_executable: impl Into<PathBuf>) -> Self {
        Self {
            gpgv_executable: gpgv_executable.into(),
        }
    }

    pub fn command_snapshot(&self) -> NodeSignatureCommandSpecV1 {
        NodeSignatureCommandSpecV1 {
            executable: self.gpgv_executable.clone(),
            args: vec![
                "--status-fd=1".into(),
                "--keyring".into(),
                "<private>/node-release-keyring.kbx".into(),
                "--output".into(),
                "<private>/SHASUMS256.txt".into(),
                "<private>/SHASUMS256.txt.asc".into(),
            ],
            environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        }
    }
}

impl NodeReleaseSignatureVerifier for ProcessNodeReleaseSignatureVerifier {
    fn verify(
        &mut self,
        armored_signature: &[u8],
        keyring: &[u8],
    ) -> Result<VerifiedNodeReleaseSignatureV1, NodeReleaseSignatureError> {
        let workspace = tempfile::Builder::new()
            .prefix("commonkit-node-signature-")
            .tempdir()
            .map_err(|error| NodeReleaseSignatureError::Unavailable(error.to_string()))?;
        let signature_path = workspace.path().join("SHASUMS256.txt.asc");
        let keyring_path = workspace.path().join("node-release-keyring.kbx");
        let output_path = workspace.path().join("SHASUMS256.txt");
        fs::write(&signature_path, armored_signature)
            .and_then(|()| fs::write(&keyring_path, keyring))
            .map_err(|error| NodeReleaseSignatureError::Unavailable(error.to_string()))?;
        let output = Command::new(&self.gpgv_executable)
            .arg("--status-fd=1")
            .arg("--keyring")
            .arg(&keyring_path)
            .arg("--output")
            .arg(&output_path)
            .arg(&signature_path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|error| NodeReleaseSignatureError::Unavailable(error.to_string()))?;
        if !output.status.success() {
            return Err(NodeReleaseSignatureError::Invalid(
                "gpgv rejected the signed checksum document".into(),
            ));
        }
        let status = String::from_utf8(output.stdout)
            .map_err(|_| NodeReleaseSignatureError::Invalid("gpgv status is not UTF-8".into()))?;
        let fingerprints = status
            .lines()
            .filter_map(|line| line.strip_prefix("[GNUPG:] VALIDSIG "))
            .filter_map(primary_validsig_fingerprint)
            .collect::<Vec<_>>();
        if fingerprints.len() != 1 {
            return Err(NodeReleaseSignatureError::Invalid(
                "gpgv did not emit exactly one valid signer".into(),
            ));
        }
        let signed_payload = fs::read(output_path)
            .map_err(|error| NodeReleaseSignatureError::Invalid(error.to_string()))?;
        Ok(VerifiedNodeReleaseSignatureV1 {
            signer_fingerprint: fingerprints[0].into(),
            signed_payload,
        })
    }
}

fn primary_validsig_fingerprint(rest: &str) -> Option<&str> {
    let fields = rest.split_whitespace().collect::<Vec<_>>();
    let fingerprint = fields.get(9).copied().or_else(|| fields.first().copied())?;
    (fingerprint.len() == 40
        && fingerprint
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then_some(fingerprint)
}

fn read_regular_no_follow(path: &Path) -> Result<Vec<u8>, NodeRuntimeHostError> {
    read_no_follow(path, false)
}

fn read_executable_no_follow(path: &Path) -> Result<Vec<u8>, NodeRuntimeHostError> {
    read_no_follow(path, true)
}

fn read_no_follow(path: &Path, require_executable: bool) -> Result<Vec<u8>, NodeRuntimeHostError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|error| NodeRuntimeHostError::Unavailable(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| NodeRuntimeHostError::Unavailable(error.to_string()))?;
    if !metadata.is_file() {
        return Err(NodeRuntimeHostError::UnsafeConfiguration(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    if require_executable {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o111 == 0 {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(format!(
                "{} is not executable",
                path.display()
            )));
        }
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| NodeRuntimeHostError::Unavailable(error.to_string()))?;
    Ok(bytes)
}

fn read_optional_regular_no_follow(path: &Path) -> Result<Option<Vec<u8>>, NodeRuntimeHostError> {
    match fs::symlink_metadata(path) {
        Ok(_) => read_regular_no_follow(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(NodeRuntimeHostError::Unavailable(error.to_string())),
    }
}

fn parse_nvm_version(bytes: &[u8]) -> Result<String, NodeRuntimeHostError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| NodeRuntimeHostError::UnsafeConfiguration("nvm.sh is not UTF-8".into()))?;
    let versions = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("NVM_VERSION="))
        .map(|value| value.trim_matches(['\'', '"']))
        .collect::<BTreeSet<_>>();
    if versions.len() != 1 {
        return Err(NodeRuntimeHostError::UnsafeConfiguration(
            "nvm.sh does not declare exactly one NVM_VERSION".into(),
        ));
    }
    Ok(versions.into_iter().next().expect("one version").into())
}

fn nvm_at_least_0_40_6(version: &str) -> bool {
    let parts = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>();
    matches!(parts.as_deref(), Ok([major, minor, patch]) if (*major, *minor, *patch) >= (0, 40, 6))
}

fn observe_installed_versions(
    nvm_dir: &Path,
) -> Result<PackageObservationV1, NodeRuntimeHostError> {
    let versions_root = nvm_dir.join("versions/node");
    let mut installed_versions = BTreeSet::new();
    let entries = match fs::read_dir(&versions_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PackageObservationV1 { installed_versions });
        }
        Err(error) => return Err(NodeRuntimeHostError::Unavailable(error.to_string())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| NodeRuntimeHostError::Unavailable(error.to_string()))?;
        let name = entry.file_name().into_string().map_err(|_| {
            NodeRuntimeHostError::UnsafeConfiguration("installed Node version is not UTF-8".into())
        })?;
        let version = name.strip_prefix('v').unwrap_or(&name);
        if version.split('.').count() != 3
            || version
                .split('.')
                .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(NodeRuntimeHostError::UnsafeConfiguration(
                "installed Node version directory is malformed".into(),
            ));
        }
        installed_versions.insert(version.into());
    }
    Ok(PackageObservationV1 { installed_versions })
}

fn host_content_digest(bytes: &[u8]) -> Result<Sha256Digest, NodeRuntimeHostError> {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(|error| NodeRuntimeHostError::Unavailable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{primary_validsig_fingerprint, safe_release_file_name};

    #[test]
    fn node_manifest_paths_are_relative_and_primary_signers_survive_subkeys() {
        assert!(safe_release_file_name("node-v22.14.0-linux-x64.tar.xz"));
        assert!(safe_release_file_name("win-x64/node.exe"));
        for unsafe_name in [
            "/node.tar.xz",
            "../node.tar.xz",
            "win-x64/../node.exe",
            "win\\node.exe",
        ] {
            assert!(!safe_release_file_name(unsafe_name));
        }

        let signing = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let primary = "5BE8A3F6C8A5C01D106C0AD820B1A390B168D356";
        let status = format!("{signing} 20260811 0 0 4 0 1 10 00 {primary}");
        assert_eq!(primary_validsig_fingerprint(&status), Some(primary));
    }
}
