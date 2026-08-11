use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use commonkit_adapters::{
    ARTIFACT_CHUNK_SIZE, OpenSshConfig, OpenSshTransport, ProcessOutput, RemoteProcessRunner,
    SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport, run_process_bounded,
};
use commonkit_core::{Sha256Digest, StableId};
use sha2::{Digest, Sha256};

#[derive(Default)]
struct FakeRunner {
    calls: Vec<(String, Vec<String>, Vec<u8>)>,
    outputs: VecDeque<ProcessOutput>,
}

impl RemoteProcessRunner for FakeRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        stdin: &[u8],
    ) -> Result<ProcessOutput, std::io::Error> {
        self.calls
            .push((program.into(), args.to_vec(), stdin.to_vec()));
        Ok(self.outputs.pop_front().expect("prepared output"))
    }
}

fn content_digest(content: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(content))).unwrap()
}

fn known_hosts_fixture() -> PathBuf {
    std::env::current_dir()
        .expect("test process has an absolute working directory")
        .join("target")
        .join("ssh-transport-fixtures")
        .join("known_hosts")
}

fn known_hosts_argument() -> String {
    known_hosts_fixture().to_string_lossy().into_owned()
}

#[test]
fn host_key_mismatch_fails_before_contacting_the_target() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"build.example.com ssh-ed25519 AAAAwrong\n".to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"256 SHA256:WRONG build.example.com (ED25519)\n".to_vec(),
        stderr: vec![],
    });
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    let error = transport
        .perform(SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        commonkit_adapters::TargetFilesystemError::HostKeyMismatch
    ));
    assert_eq!(transport.into_runner().calls.len(), 2);
}

#[test]
fn unrelated_matching_pin_cannot_authorize_a_wrong_target_key() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"build.example.com ssh-ed25519 AAAAtarget-wrong\n".to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"256 SHA256:WRONG build.example.com (ED25519)\n".to_vec(),
        stderr: vec![],
    });
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    let error = transport
        .perform(SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        commonkit_adapters::TargetFilesystemError::HostKeyMismatch
    ));
    let calls = transport.into_runner().calls;
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].1,
        vec!["-F", "build.example.com", "-f", &known_hosts_argument()]
    );
}

#[test]
fn non_default_port_uses_bracketed_known_hosts_identity() {
    let config = OpenSshConfig::new(
        "build.example.com",
        "commonkit",
        2222,
        known_hosts_fixture(),
        "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap();
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 1,
        stdout: vec![],
        stderr: vec![],
    });
    let mut transport = OpenSshTransport::new(config, runner).unwrap();
    let _ = transport.perform(SshFilesystemRequest::ReadFile {
        root_id: StableId::parse("home").unwrap(),
        path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
    });
    assert_eq!(
        transport.into_runner().calls[0].1,
        vec![
            "-F",
            "[build.example.com]:2222",
            "-f",
            &known_hosts_argument(),
        ]
    );
}

#[test]
fn duplicate_matching_target_keys_fail_as_ambiguous() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"build.example.com ssh-ed25519 AAAAfirst\nbuild.example.com ssh-ed25519 AAAAduplicate\n".to_vec(),
        stderr: vec![],
    });
    for _ in 0..2 {
        runner.outputs.push_back(ProcessOutput {
            status: 0,
            stdout: b"256 SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA build.example.com (ED25519)\n".to_vec(),
            stderr: vec![],
        });
    }
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    let error = transport
        .perform(SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        commonkit_adapters::TargetFilesystemError::AmbiguousHostKeyPin
    ));
    assert_eq!(transport.into_runner().calls.len(), 3);
}

#[test]
fn wildcard_record_is_not_treated_as_the_exact_target_identity() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"*.example.com ssh-ed25519 AAAAkey\n".to_vec(),
        stderr: vec![],
    });
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    let error = transport
        .perform(SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
        })
        .unwrap_err();
    assert!(matches!(
        error,
        commonkit_adapters::TargetFilesystemError::HostKeyMismatch
    ));
    assert_eq!(transport.into_runner().calls.len(), 1);
}

#[test]
fn hashed_exact_host_record_is_preserved_for_strict_ssh_checking() {
    let hashed_record = "|1|salt|hash ssh-ed25519 AAAAcorrect";
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: format!("# Host build.example.com found: line 1\n{hashed_record}\n").into_bytes(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout:
            b"256 SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA build.example.com (ED25519)\n"
                .to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: serde_json::to_vec(&SshFilesystemResponse::Absent).unwrap(),
        stderr: vec![],
    });
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    assert_eq!(
        transport
            .perform(SshFilesystemRequest::ReadFile {
                root_id: StableId::parse("home").unwrap(),
                path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
            })
            .unwrap(),
        SshFilesystemResponse::Absent
    );
    let calls = transport.into_runner().calls;
    assert_eq!(calls[1].2, format!("{hashed_record}\n").as_bytes());
    assert!(
        calls[2]
            .1
            .iter()
            .any(|arg| arg == "StrictHostKeyChecking=yes")
    );
    assert!(
        calls[2]
            .1
            .windows(2)
            .any(|pair| pair == ["-o", "GlobalKnownHostsFile=none"]),
        "system-wide trust stores must not bypass the configured exact pin: {:?}",
        calls[2].1
    );
}

#[test]
fn remote_failures_do_not_expose_remote_stderr() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"build.example.com ssh-ed25519 AAAAcorrect\n".to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout:
            b"256 SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA build.example.com (ED25519)\n"
                .to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 23,
        stdout: vec![],
        stderr: b"token=super-secret-value".to_vec(),
    });
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    let error = transport
        .perform(SshFilesystemRequest::ReadFile {
            root_id: StableId::parse("home").unwrap(),
            path: commonkit_adapters::NormalizedManagedPath::parse(".config/tool").unwrap(),
        })
        .unwrap_err();
    let rendered = error.to_string();
    assert!(!rendered.contains("super-secret-value"));
    assert_eq!(
        rendered,
        "remote helper failed (23): remote process reported an error"
    );
}

fn config() -> OpenSshConfig {
    OpenSshConfig::new(
        "build.example.com",
        "commonkit",
        22,
        known_hosts_fixture(),
        "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap()
}

struct LocalHelperRunner {
    home: PathBuf,
    drop_final_response: bool,
}

impl RemoteProcessRunner for LocalHelperRunner {
    fn run(
        &mut self,
        program: &str,
        _args: &[String],
        stdin: &[u8],
    ) -> Result<ProcessOutput, std::io::Error> {
        if program == "ssh-keygen" {
            return Ok(if stdin.is_empty() {
                ProcessOutput {
                    status: 0,
                    stdout: b"build.example.com ssh-ed25519 AAAAcorrect\n".to_vec(),
                    stderr: vec![],
                }
            } else {
                ProcessOutput {
                    status: 0,
                    stdout: b"256 SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA build.example.com (ED25519)\n".to_vec(),
                    stderr: vec![],
                }
            });
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonkit-target-helper"))
            .arg("--stdio-v1")
            .env("HOME", &self.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().expect("helper stdin").write_all(stdin)?;
        let output = child.wait_with_output()?;
        if self.drop_final_response
            && serde_json::from_slice::<SshFilesystemRequest>(stdin)
                .ok()
                .is_some_and(|request| {
                    matches!(
                        request,
                        SshFilesystemRequest::StageArtifactChunk {
                            sequence: 1,
                            total_chunks: 2,
                            ..
                        }
                    )
                })
        {
            self.drop_final_response = false;
            return Ok(ProcessOutput {
                status: 1,
                stdout: vec![],
                stderr: vec![],
            });
        }
        Ok(ProcessOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

#[test]
fn process_backed_transport_round_trips_chunked_artifact_without_monolithic_json() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let target = temporary.path().join("target");
    let state = temporary.path().join("state");
    std::fs::create_dir_all(home.join(".config/commonkit")).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(
        home.join(".config/commonkit/target-helper.json"),
        serde_json::to_vec(&serde_json::json!({
            "stateRoot": state,
            "roots": [{"id":"home","path":target,"access":"read_write"}]
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            home.join(".config/commonkit/target-helper.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let bytes = vec![b'x'; ARTIFACT_CHUNK_SIZE as usize + 1];
    let digest = content_digest(&bytes);
    let transfer = StableId::parse("transport-artifact").unwrap();
    let mut transport = OpenSshTransport::new(
        config(),
        LocalHelperRunner {
            home,
            drop_final_response: true,
        },
    )
    .unwrap();
    for (sequence, content) in [
        bytes[..ARTIFACT_CHUNK_SIZE as usize].to_vec(),
        bytes[ARTIFACT_CHUNK_SIZE as usize..].to_vec(),
    ]
    .into_iter()
    .enumerate()
    {
        let offset = sequence as u64 * ARTIFACT_CHUNK_SIZE as u64;
        let request = SshFilesystemRequest::StageArtifactChunk {
            run_id: transfer.clone(),
            transfer_id: transfer.clone(),
            digest: digest.clone(),
            byte_count: bytes.len() as u64,
            chunk_size: ARTIFACT_CHUNK_SIZE,
            sequence: sequence as u32,
            offset,
            total_chunks: 2,
            content,
        };
        if sequence == 1 {
            assert!(transport.perform(request.clone()).is_err());
        }
        let response = transport.perform(request).unwrap();
        assert!(matches!(
            response,
            SshFilesystemResponse::ArtifactChunkStaged { .. }
        ));
    }
    let verified = transport
        .perform(SshFilesystemRequest::VerifyArtifact {
            run_id: transfer.clone(),
            digest: digest.clone(),
        })
        .unwrap();
    assert_eq!(
        verified,
        SshFilesystemResponse::ArtifactVerified {
            digest: digest.clone()
        }
    );
    let response = transport
        .perform(SshFilesystemRequest::ReadArtifactChunk {
            request_id: StableId::parse("resolution-request").unwrap(),
            transfer_id: transfer,
            digest,
            byte_count: bytes.len() as u64,
            chunk_size: ARTIFACT_CHUNK_SIZE,
            sequence: 1,
            offset: ARTIFACT_CHUNK_SIZE as u64,
            total_chunks: 2,
        })
        .unwrap();
    assert!(
        matches!(response, SshFilesystemResponse::ArtifactChunk { content, .. } if content == vec![b'x'])
    );
}

#[test]
fn pinned_transport_uses_fixed_argv_and_typed_stdio_protocol() {
    let artifact_digest = content_digest(b"payload");
    let response = SshFilesystemResponse::ArtifactStaged {
        digest: artifact_digest.clone(),
    };
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"build.example.com ssh-ed25519 AAAAcorrect\n".to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout:
            b"256 SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA build.example.com (ED25519)\n"
                .to_vec(),
        stderr: vec![],
    });
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: serde_json::to_vec(&response).unwrap(),
        stderr: vec![],
    });
    let mut transport = OpenSshTransport::new(config(), runner).unwrap();
    let request = SshFilesystemRequest::StageArtifact {
        run_id: StableId::parse("run-1").unwrap(),
        digest: artifact_digest,
        content: b"payload".to_vec(),
    };

    assert_eq!(transport.perform(request.clone()).unwrap(), response);
    let runner = transport.into_runner();
    assert_eq!(runner.calls.len(), 3);
    assert_eq!(runner.calls[0].0, "ssh-keygen");
    assert_eq!(
        runner.calls[0].1,
        vec!["-F", "build.example.com", "-f", &known_hosts_argument()]
    );
    assert_eq!(runner.calls[1].0, "ssh-keygen");
    assert_eq!(runner.calls[1].1, vec!["-lf", "-", "-E", "sha256"]);
    assert_eq!(
        runner.calls[1].2,
        b"build.example.com ssh-ed25519 AAAAcorrect\n"
    );
    assert_eq!(runner.calls[2].0, "ssh");
    assert_eq!(
        runner.calls[2].1,
        vec![
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "GlobalKnownHostsFile=none",
            "-o",
            runner.calls[2].1[10].as_str(),
            "-p",
            "22",
            "--",
            "commonkit@build.example.com",
            "commonkit-target-helper",
            "--stdio-v1",
        ]
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemRequest>(&runner.calls[2].2).unwrap(),
        request
    );
    assert!(runner.calls[2].1[10].starts_with("UserKnownHostsFile="));
    assert_ne!(
        runner.calls[2].1[8],
        format!("UserKnownHostsFile={}", known_hosts_argument())
    );
}

#[test]
fn process_runner_kills_bounded_stdout_overflow_without_pipe_deadlock() {
    let started = std::time::Instant::now();
    let error =
        run_process_bounded("/bin/sh", &["-c".into(), "yes x".into()], &[], 16).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn process_runner_drains_and_kills_bounded_stderr_overflow() {
    let started = std::time::Instant::now();
    let error =
        run_process_bounded("/bin/sh", &["-c".into(), "yes x >&2".into()], &[], 16).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn process_runner_kills_descendants_holding_overflow_pipe() {
    let started = std::time::Instant::now();
    let error = run_process_bounded(
        "/bin/sh",
        &["-c".into(), "sleep 30 & yes x >&2".into()],
        &[],
        16,
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
