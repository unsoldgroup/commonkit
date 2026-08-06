use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use commonkit_contracts::Principal;

const PRINCIPAL_OUTPUT_LIMIT: usize = 64;

pub struct CommandOutput {
    success: bool,
    stdout: Vec<u8>,
}

impl CommandOutput {
    pub fn success(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            success: true,
            stdout: stdout.into(),
        }
    }

    pub fn failure(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            success: false,
            stdout: stdout.into(),
        }
    }
}

pub trait PrincipalCommandRunner {
    fn run(&mut self, arguments: &[&str]) -> io::Result<CommandOutput>;
}

pub struct ProcessPrincipalCommandRunner;

impl PrincipalCommandRunner for ProcessPrincipalCommandRunner {
    fn run(&mut self, arguments: &[&str]) -> io::Result<CommandOutput> {
        let output = Command::new(github_cli_executable()?)
            .args(arguments)
            .output()?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
        })
    }
}

pub fn resolve_principal() -> Option<Principal> {
    resolve_principal_with(&mut ProcessPrincipalCommandRunner)
}

pub fn resolve_principal_with(runner: &mut impl PrincipalCommandRunner) -> Option<Principal> {
    let output = runner
        .run(&["api", "--hostname", "github.com", "user", "--jq", ".login"])
        .ok()?;
    if !output.success || output.stdout.len() > PRINCIPAL_OUTPUT_LIMIT {
        return None;
    }
    let login = std::str::from_utf8(&output.stdout).ok()?.trim();
    Principal::parse(login).ok()
}

pub fn github_cli_executable() -> io::Result<PathBuf> {
    github_cli_candidates(std::env::var_os("PATH"))
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "GitHub CLI executable not found"))
}

fn github_cli_candidates(path: Option<OsString>) -> Vec<PathBuf> {
    let executable = format!("gh{}", std::env::consts::EXE_SUFFIX);
    let mut candidates = path
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .map(|directory| directory.join(&executable))
        .collect::<Vec<_>>();
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/gh"),
        PathBuf::from("/usr/local/bin/gh"),
    ]);
    #[cfg(target_os = "linux")]
    candidates.extend([
        PathBuf::from("/usr/bin/gh"),
        PathBuf::from("/usr/local/bin/gh"),
        PathBuf::from("/snap/bin/gh"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/gh"),
    ]);
    #[cfg(target_os = "windows")]
    {
        if let Some(root) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(root).join("GitHub CLI/gh.exe"));
        }
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(root).join("GitHub CLI/gh.exe"));
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_cli_discovery_keeps_fixed_executable_names_and_path_order() {
        let path = std::env::join_paths([PathBuf::from("/first"), PathBuf::from("/second")])
            .expect("portable path list");
        let candidates = github_cli_candidates(Some(path));
        let executable = format!("gh{}", std::env::consts::EXE_SUFFIX);

        assert_eq!(candidates[0], PathBuf::from("/first").join(&executable));
        assert_eq!(candidates[1], PathBuf::from("/second").join(executable));
        #[cfg(target_os = "macos")]
        assert_eq!(
            &candidates[2..],
            [
                PathBuf::from("/opt/homebrew/bin/gh"),
                PathBuf::from("/usr/local/bin/gh"),
            ]
        );
    }
}
