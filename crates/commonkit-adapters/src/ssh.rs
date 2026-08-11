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
        if host.is_empty()
            || host.starts_with('-')
            || host.chars().any(char::is_whitespace)
            || host
                .chars()
                .any(|character| matches!(character, ',' | '[' | ']' | '*' | '?' | '!' | '|'))
        {
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
    pinned_known_hosts: Option<tempfile::NamedTempFile>,
}

impl<R: RemoteProcessRunner> OpenSshTransport<R> {
    pub fn new(config: OpenSshConfig, runner: R) -> Result<Self, TargetFilesystemError> {
        Ok(Self {
            config,
            runner,
            host_verified: false,
            pinned_known_hosts: None,
        })
    }
    pub fn into_runner(self) -> R {
        self.runner
    }

    fn verify_host_key(&mut self) -> Result<(), TargetFilesystemError> {
        if self.host_verified {
            return Ok(());
        }
        let host_token = if self.config.port == 22 {
            self.config.host.clone()
        } else {
            format!("[{}]:{}", self.config.host, self.config.port)
        };
        let lookup_args = vec![
            "-F".into(),
            host_token.clone(),
            "-f".into(),
            self.config.known_hosts.to_string_lossy().into_owned(),
        ];
        let lookup = self.runner.run("ssh-keygen", &lookup_args, &[])?;
        if lookup.status != 0 {
            return Err(TargetFilesystemError::HostKeyMismatch);
        }

        // `ssh-keygen -F` resolves both cleartext and hashed known_hosts entries.
        // Fingerprint each returned key independently so a matching key for an
        // unrelated destination can never satisfy this pin.
        let records = String::from_utf8_lossy(&lookup.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut matching = Vec::new();
        for record in records {
            let fields = record.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 || fields[0].starts_with('@') {
                return Err(TargetFilesystemError::HostKeyMismatch);
            }
            let recorded_hosts = fields[0];
            if !recorded_hosts.starts_with("|1|")
                && !recorded_hosts
                    .split(',')
                    .any(|candidate| candidate == host_token)
            {
                return Err(TargetFilesystemError::HostKeyMismatch);
            }
            let fingerprint_args = vec!["-lf".into(), "-".into(), "-E".into(), "sha256".into()];
            let fingerprint = self.runner.run(
                "ssh-keygen",
                &fingerprint_args,
                format!("{record}\n").as_bytes(),
            )?;
            if fingerprint.status == 0
                && String::from_utf8_lossy(&fingerprint.stdout)
                    .split_whitespace()
                    .any(|field| field == self.config.fingerprint)
            {
                matching.push(record);
            }
        }
        if matching.len() != 1 {
            return Err(if matching.is_empty() {
                TargetFilesystemError::HostKeyMismatch
            } else {
                TargetFilesystemError::AmbiguousHostKeyPin
            });
        }

        let mut pinned = tempfile::NamedTempFile::new()?;
        writeln!(pinned, "{}", matching[0])?;
        pinned.flush()?;
        self.pinned_known_hosts = Some(pinned);
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
            "GlobalKnownHostsFile=none".into(),
            "-o".into(),
            format!(
                "UserKnownHostsFile={}",
                self.pinned_known_hosts
                    .as_ref()
                    .expect("host verification creates isolated known_hosts")
                    .path()
                    .to_string_lossy()
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
            (
                SshFilesystemRequest::PackageMutation { phase, .. },
                SshFilesystemResponse::PackageObserved { .. },
            ) if *phase == crate::PackageMutationPhase::Observe => true,
            (
                SshFilesystemRequest::PackageMutation { phase, .. },
                SshFilesystemResponse::Applied,
            ) if *phase != crate::PackageMutationPhase::Observe => true,
            (
                SshFilesystemRequest::PackageResolution { .. },
                SshFilesystemResponse::PackageResolution { .. }
                | SshFilesystemResponse::PackageResolutionRejected { .. },
            ) => true,
            _ => false,
        };
        if !matching {
            return Err(TargetFilesystemError::InvalidRemoteResponse);
        }
        Ok(response)
    }
}
