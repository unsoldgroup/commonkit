use std::collections::VecDeque;
use std::path::PathBuf;

use commonkit_adapters::{
    OpenSshConfig, OpenSshTransport, ProcessOutput, RemoteProcessRunner, SshFilesystemRequest,
    SshFilesystemResponse, SshFilesystemTransport,
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

#[test]
fn host_key_mismatch_fails_before_contacting_the_target() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"256 SHA256:WRONG host (ED25519)\n".to_vec(),
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
fn remote_failures_do_not_expose_remote_stderr() {
    let mut runner = FakeRunner::default();
    runner.outputs.push_back(ProcessOutput {
        status: 0,
        stdout: b"256 SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA host (ED25519)\n".to_vec(),
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
        PathBuf::from("/state/known_hosts"),
        "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap()
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
    assert_eq!(runner.calls.len(), 2);
    assert_eq!(runner.calls[0].0, "ssh-keygen");
    assert_eq!(runner.calls[1].0, "ssh");
    assert_eq!(
        runner.calls[1].1,
        vec![
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "UserKnownHostsFile=/state/known_hosts",
            "-p",
            "22",
            "--",
            "commonkit@build.example.com",
            "commonkit-target-helper",
            "--stdio-v1",
        ]
    );
    assert_eq!(
        serde_json::from_slice::<SshFilesystemRequest>(&runner.calls[1].2).unwrap(),
        request
    );
}
