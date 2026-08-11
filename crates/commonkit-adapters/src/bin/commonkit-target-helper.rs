use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use commonkit_adapters::{
    AptRepositoryConfigurationV1, NodeRuntimeHost, ProcessAptResolutionCommandRunner,
    ProcessNodeRuntimeHost, SshFilesystemRequest, TargetHelper, TargetNodeResolutionConfig,
    TargetPackageResolutionConfig,
};
use commonkit_contracts::{SecurityPolicy, Sha256Digest, StableId};
use commonkit_core::TargetRoot;
use serde::Deserialize;

const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperConfig {
    state_root: PathBuf,
    roots: Vec<TargetRoot>,
    #[serde(default)]
    package_resolution: Option<TargetPackageResolutionConfig>,
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let result = match args.get(1).map(String::as_str) {
        Some("--stdio-v1") if args.len() == 2 => run(),
        Some("--probe-package-resolution") => provision(&args[2..]),
        _ => Err(()),
    };
    if result.is_err() {
        fail();
    }
}

fn run() -> Result<(), ()> {
    let home = std::env::var_os("HOME").ok_or(())?;
    let config_path = PathBuf::from(home).join(".config/commonkit/target-helper.json");
    let metadata = std::fs::symlink_metadata(&config_path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        if mode & 0o022 != 0 {
            return Err(());
        }
        let effective_uid = unsafe { libc::geteuid() };
        let owner_allowed =
            metadata.uid() == effective_uid || (effective_uid == 0 && metadata.uid() == 0);
        if !owner_allowed {
            return Err(());
        }
        let mut parent = config_path.clone();
        parent.pop();
        for _ in 0..2 {
            let parent_metadata = std::fs::symlink_metadata(&parent).map_err(|_| ())?;
            if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
                return Err(());
            }
            parent.pop();
        }
    }
    let config_bytes = std::fs::read(&config_path).map_err(|_| ())?;
    if config_bytes.len() > 1024 * 1024 {
        return Err(());
    }
    let config: HelperConfig = serde_json::from_slice(&config_bytes).map_err(|_| ())?;
    let helper = TargetHelper::open_with_package_resolution(
        config.roots,
        &config.state_root,
        config.package_resolution,
    )
    .map_err(|_| ())?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|_| ())?;
    if input.len() as u64 > MAX_REQUEST_BYTES {
        return Err(());
    }
    let request: SshFilesystemRequest = serde_json::from_slice(&input).map_err(|_| ())?;
    let response = helper.dispatch(request).map_err(|_| ())?;
    serde_json::to_writer(std::io::stdout().lock(), &response).map_err(|_| ())?;
    std::io::stdout().lock().flush().map_err(|_| ())
}

fn option(args: &[String], name: &str) -> Result<String, ()> {
    let index = args.iter().position(|arg| arg == name).ok_or(())?;
    args.get(index + 1).cloned().ok_or(())
}

fn target_probe() -> Result<commonkit_adapters::PackageTargetV1, ()> {
    let os_release = fs::read_to_string("/etc/os-release").map_err(|_| ())?;
    let fields = os_release
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key, value.trim_matches('"')))
        .collect::<BTreeMap<_, _>>();
    let os_version = fields.get("VERSION_ID").copied().ok_or(())?;
    let codename = fields
        .get("VERSION_CODENAME")
        .or_else(|| fields.get("UBUNTU_CODENAME"))
        .map(|value| (*value).to_owned());
    let distro_id = fields.get("ID").map(|value| (*value).to_owned());
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return Err(()),
    };
    Ok(commonkit_adapters::PackageTargetV1 {
        os: "linux".into(),
        os_version: os_version.into(),
        distro_id,
        distro_version: Some(os_version.into()),
        codename,
        arch: arch.into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    })
}

fn canonical_apt_source(source: &StableId) -> Result<&'static str, ()> {
    match source.as_str() {
        "ubuntu-main" => Ok("https://archive.ubuntu.com/ubuntu"),
        "debian-main" => Ok("https://deb.debian.org/debian"),
        _ => Err(()),
    }
}

fn package_policy(source: &StableId) -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            StableId::parse("package_sources").expect("static id"),
            BTreeSet::from([source.as_str().to_owned()]),
        )]),
        ..SecurityPolicy::default()
    }
}

fn provision(args: &[String]) -> Result<(), ()> {
    let config_path = option(args, "--config").map(PathBuf::from).or_else(|_| {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/commonkit/target-helper.json"))
            .ok_or(())
    })?;
    let target_identity_digest =
        Sha256Digest::parse(option(args, "--target-identity-digest")?).map_err(|_| ())?;
    let manager = option(args, "--manager")?;
    let base = read_or_create_config(&config_path, args)?;
    let target = target_probe()?;
    let (binding, apt, node, source) = match manager.as_str() {
        "apt" => {
            let source_id = StableId::parse(option(args, "--apt-source-id")?).map_err(|_| ())?;
            let suite = option(args, "--apt-suite")?;
            let components = option(args, "--apt-components")?
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            if components.is_empty() {
                return Err(());
            }
            let signed_by = PathBuf::from(option(args, "--apt-signed-by")?);
            let signing_authority =
                StableId::parse(option(args, "--apt-signing-authority")?).map_err(|_| ())?;
            let repository = AptRepositoryConfigurationV1 {
                source_id: source_id.clone(),
                suite,
                components,
                signed_by,
                signing_authority,
            };
            let target = target.clone();
            let canonical = canonical_apt_source(&source_id)?;
            let binding = ProcessAptResolutionCommandRunner::probe_manager_binding(
                &target,
                canonical,
                &repository,
            )
            .map_err(|_| ())?;
            (binding, Some(repository), None, source_id)
        }
        "nvm" => {
            let nvm_dir = PathBuf::from(option(args, "--nvm-dir")?);
            let shell = PathBuf::from(option(args, "--shell-executable")?);
            let keyring = PathBuf::from(option(args, "--release-keyring")?);
            let gpgv = PathBuf::from(option(args, "--gpgv-executable")?);
            let mut target = target.clone();
            target.manager_prefix = Some(nvm_dir.to_string_lossy().into_owned());
            let mut host = ProcessNodeRuntimeHost::new_with_gpgv(
                nvm_dir.clone(),
                shell.clone(),
                keyring.clone(),
                gpgv.clone(),
            );
            let snapshot = host.probe(&target).map_err(|_| ())?;
            let node = TargetNodeResolutionConfig {
                nvm_dir,
                shell_executable: shell,
                release_keyring: keyring,
                gpgv_executable: gpgv,
                gpgv_executable_digest: snapshot.gpgv_executable_digest,
            };
            (
                snapshot.manager,
                None,
                Some(node),
                StableId::parse("nodejs-nvm").unwrap(),
            )
        }
        _ => return Err(()),
    };
    let mut config = base;
    config.package_resolution = Some(TargetPackageResolutionConfig {
        target,
        manager: binding,
        policy: package_policy(&source),
        apt,
        node,
        target_identity_digest,
    });
    write_config_atomic(&config_path, &config)
}

fn read_or_create_config(path: &Path, args: &[String]) -> Result<HelperConfig, ()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.permissions().mode() & 0o022 != 0
                || metadata.uid() != unsafe { libc::geteuid() }
            {
                return Err(());
            }
        }
        let bytes = fs::read(path).map_err(|_| ())?;
        return serde_json::from_slice(&bytes).map_err(|_| ());
    }
    let state_root = PathBuf::from(option(args, "--state-root")?);
    let root_path = PathBuf::from(option(args, "--root-path")?);
    let root_id = StableId::parse(option(args, "--root-id")?).map_err(|_| ())?;
    Ok(HelperConfig {
        state_root,
        roots: vec![TargetRoot {
            id: root_id,
            path: root_path.to_string_lossy().into_owned(),
            access: commonkit_core::RootAccess::ReadWrite,
        }],
        package_resolution: None,
    })
}

fn write_config_atomic(path: &Path, config: &HelperConfig) -> Result<(), ()> {
    let parent = path.parent().ok_or(())?;
    let mut current = Some(parent);
    while let Some(directory) = current {
        let metadata = fs::symlink_metadata(directory).map_err(|_| ())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(());
        }
        current = directory.parent();
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(());
        }
    }
    let temp = parent.join(format!(".target-helper.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(config).map_err(|_| ())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| ())?;
    }
    file.write_all(&bytes).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())?;
    fs::rename(&temp, path).map_err(|_| ())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o022 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(());
        }
    }
    let verified: HelperConfig =
        serde_json::from_slice(&fs::read(path).map_err(|_| ())?).map_err(|_| ())?;
    TargetHelper::open_with_package_resolution(
        verified.roots,
        &verified.state_root,
        verified.package_resolution,
    )
    .map_err(|_| ())?;
    Ok(())
}

fn fail() -> ! {
    eprintln!("commonkit target helper rejected the request");
    std::process::exit(1)
}
