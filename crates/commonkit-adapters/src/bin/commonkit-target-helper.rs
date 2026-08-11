use std::io::{Read, Write};
use std::path::PathBuf;

use commonkit_adapters::{SshFilesystemRequest, TargetHelper, TargetPackageResolutionConfig};
use commonkit_core::TargetRoot;
use serde::Deserialize;

const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperConfig {
    state_root: PathBuf,
    roots: Vec<TargetRoot>,
    #[serde(default)]
    package_resolution: Option<TargetPackageResolutionConfig>,
}

fn main() {
    if std::env::args()
        .collect::<Vec<_>>()
        .as_slice()
        .get(1)
        .map(String::as_str)
        != Some("--stdio-v1")
        || std::env::args().count() != 2
    {
        fail();
    }
    if run().is_err() {
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

fn fail() -> ! {
    eprintln!("commonkit target helper rejected the request");
    std::process::exit(1)
}
