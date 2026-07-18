use std::io::{Read, Write};
use std::path::PathBuf;

use commonkit_adapters::{SshFilesystemRequest, TargetHelper};
use commonkit_core::TargetRoot;
use serde::Deserialize;

const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperConfig {
    state_root: PathBuf,
    roots: Vec<TargetRoot>,
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
    let config: HelperConfig =
        serde_json::from_slice(&std::fs::read(config_path).map_err(|_| ())?).map_err(|_| ())?;
    let helper = TargetHelper::open(config.roots, &config.state_root).map_err(|_| ())?;
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
