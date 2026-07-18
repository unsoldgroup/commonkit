use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use commonkit_contracts::Sha256Digest;
use sha2::{Digest, Sha256};

use crate::{
    SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport, TargetFilesystemError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshConfig {
    host: String,
    user: String,
    port: u16,
    known_hosts: PathBuf,
    fingerprint: String,
}

impl OpenSshConfig {
    pub fn new(
        host: impl Into<String>,
        user: impl Into<String>,
        port: u16,
        known_hosts: PathBuf,
        fingerprint: impl Into<String>,
    ) -> Result<Self, TargetFilesystemError> {
        let host = host.into();
        let user = user.into();
        let fingerprint = fingerprint.into();
        if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
            return Err(TargetFilesystemError::InvalidSshConfig("invalid host"));
        }
        if user.is_empty()
            || user.starts_with('-')
            || !user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            return Err(TargetFilesystemError::InvalidSshConfig("invalid user"));
        }
        if port == 0
            || !known_hosts.is_absolute()
            || !fingerprint.starts_with("SHA256:")
            || fingerprint.len() < 20
        {
            return Err(TargetFilesystemError::InvalidSshConfig(
                "invalid pinning configuration",
            ));
        }
        Ok(Self {
            host,
            user,
            port,
            known_hosts,
            fingerprint,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait RemoteProcessRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        stdin: &[u8],
    ) -> Result<ProcessOutput, std::io::Error>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessRemoteRunner;

impl RemoteProcessRunner for ProcessRemoteRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        stdin: &[u8],
    ) -> Result<ProcessOutput, std::io::Error> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().expect("piped stdin").write_all(stdin)?;
        let output = child.wait_with_output()?;
        Ok(ProcessOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub struct OpenSshTransport<R> {
    config: OpenSshConfig,
    runner: R,
    host_verified: bool,
}

impl<R: RemoteProcessRunner> OpenSshTransport<R> {
    pub fn new(config: OpenSshConfig, runner: R) -> Result<Self, TargetFilesystemError> {
        Ok(Self {
            config,
            runner,
            host_verified: false,
        })
    }
    pub fn into_runner(self) -> R {
        self.runner
    }

    fn verify_host_key(&mut self) -> Result<(), TargetFilesystemError> {
        if self.host_verified {
            return Ok(());
        }
        let args = vec![
            "-lf".into(),
            self.config.known_hosts.to_string_lossy().into_owned(),
            "-E".into(),
            "sha256".into(),
        ];
        let out = self.runner.run("ssh-keygen", &args, &[])?;
        if out.status != 0
            || !String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .any(|field| field == self.config.fingerprint)
        {
            return Err(TargetFilesystemError::HostKeyMismatch);
        }
        self.host_verified = true;
        Ok(())
    }

    fn ssh_args(&self) -> Vec<String> {
        vec![
            "-T".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            format!(
                "UserKnownHostsFile={}",
                self.config.known_hosts.to_string_lossy()
            ),
            "-p".into(),
            self.config.port.to_string(),
            "--".into(),
            format!("{}@{}", self.config.user, self.config.host),
            "commonkit-target-helper".into(),
            "--stdio-v1".into(),
        ]
    }
}

impl<R: RemoteProcessRunner> SshFilesystemTransport for OpenSshTransport<R> {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        self.verify_host_key()?;
        if let SshFilesystemRequest::StageArtifact {
            digest, content, ..
        } = &request
        {
            let actual = Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(content)))
                .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?;
            if &actual != digest {
                return Err(TargetFilesystemError::InvalidRemoteResponse);
            }
        }
        let input = serde_json::to_vec(&request)
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?;
        let out = self.runner.run("ssh", &self.ssh_args(), &input)?;
        if out.status != 0 {
            return Err(TargetFilesystemError::RemoteFailure {
                status: out.status,
                message: "remote process reported an error".into(),
            });
        }
        let response: SshFilesystemResponse = serde_json::from_slice(&out.stdout)
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?;
        let matching = match (&request, &response) {
            (
                SshFilesystemRequest::StageArtifact { digest: a, .. },
                SshFilesystemResponse::ArtifactStaged { digest: b },
            ) => a == b,
            (
                SshFilesystemRequest::VerifyArtifact { digest: a, .. },
                SshFilesystemResponse::ArtifactVerified { digest: b },
            ) => a == b,
            (
                SshFilesystemRequest::BindRecoveryReceipt {
                    receipt_digest: a, ..
                },
                SshFilesystemResponse::RecoveryReceiptBound { receipt_digest: b },
            ) => a == b,
            (
                SshFilesystemRequest::RecoverRun {
                    receipt_digest: a, ..
                },
                SshFilesystemResponse::RecoveryReady { receipt_digest: b },
            ) => a == b,
            (
                SshFilesystemRequest::ReadFile { .. },
                SshFilesystemResponse::Absent | SshFilesystemResponse::File { .. },
            ) => true,
            (
                SshFilesystemRequest::WriteFile { .. } | SshFilesystemRequest::Remove { .. },
                SshFilesystemResponse::Applied,
            ) => true,
            _ => false,
        };
        if !matching {
            return Err(TargetFilesystemError::InvalidRemoteResponse);
        }
        Ok(response)
    }
}
