use std::{collections::BTreeMap, path::PathBuf};

use commonkit_contracts::Sha256Digest;
use commonkit_relay::{
    LocalSecretResolver, LocalStdioError, LocalStdioLimits, LocalStdioProcessManager,
    LocalStdioUpstream, LocalStdioValidationError, ResolvedSecret,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn digest() -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap()
}

struct NoSecrets;

impl LocalSecretResolver for NoSecrets {
    fn resolve(&self, _: &str) -> Result<ResolvedSecret, LocalStdioError> {
        panic!("no secret references expected")
    }
}

fn executable_digest(path: &str) -> Sha256Digest {
    Sha256Digest::parse(format!(
        "sha256:{:x}",
        Sha256::digest(std::fs::read(path).unwrap())
    ))
    .unwrap()
}

#[test]
fn local_stdio_requires_an_absolute_allowlisted_executable_and_secret_references() {
    let limits = LocalStdioLimits {
        startup_timeout_ms: 5_000,
        call_timeout_ms: 30_000,
        max_request_bytes: 64 * 1024,
        max_response_bytes: 256 * 1024,
        max_restarts: 3,
    };
    let valid = LocalStdioUpstream::new(
        PathBuf::from("/usr/local/bin/context-server"),
        digest(),
        vec!["--stdio".into()],
        BTreeMap::from([("API_TOKEN".into(), "bws:shared/context-token".into())]),
        limits,
    )
    .unwrap();
    assert_eq!(
        valid.executable(),
        PathBuf::from("/usr/local/bin/context-server")
    );

    assert_eq!(
        LocalStdioUpstream::new(
            PathBuf::from("context-server"),
            digest(),
            vec![],
            BTreeMap::new(),
            limits,
        )
        .unwrap_err(),
        LocalStdioValidationError::ExecutableMustBeAbsolute
    );
    assert_eq!(
        LocalStdioUpstream::new(
            PathBuf::from("/usr/local/bin/context-server"),
            digest(),
            vec![],
            BTreeMap::from([("API_TOKEN".into(), "plaintext-token".into())]),
            limits,
        )
        .unwrap_err(),
        LocalStdioValidationError::LiteralEnvironmentSecret
    );
}

#[tokio::test]
async fn local_stdio_process_is_digest_bound_persistent_and_response_bounded() {
    let limits = LocalStdioLimits {
        startup_timeout_ms: 5_000,
        call_timeout_ms: 5_000,
        max_request_bytes: 32,
        max_response_bytes: 32,
        max_restarts: 1,
    };
    let upstream = LocalStdioUpstream::new(
        PathBuf::from("/bin/cat"),
        executable_digest("/bin/cat"),
        vec![],
        BTreeMap::new(),
        limits,
    )
    .unwrap();
    let manager = LocalStdioProcessManager::new(upstream, Arc::new(NoSecrets));
    assert_eq!(manager.call(br#"{"id":1}"#).await.unwrap(), br#"{"id":1}"#);
    assert_eq!(manager.call(br#"{"id":2}"#).await.unwrap(), br#"{"id":2}"#);
    assert!(matches!(
        manager.call(&[b'a'; 33]).await,
        Err(LocalStdioError::RequestTooLarge)
    ));
    manager.stop().await;
}

#[tokio::test]
async fn local_stdio_refuses_a_changed_executable_before_spawning() {
    let upstream = LocalStdioUpstream::new(
        PathBuf::from("/bin/cat"),
        digest(),
        vec![],
        BTreeMap::new(),
        LocalStdioLimits {
            startup_timeout_ms: 1_000,
            call_timeout_ms: 1_000,
            max_request_bytes: 32,
            max_response_bytes: 32,
            max_restarts: 0,
        },
    )
    .unwrap();
    let manager = LocalStdioProcessManager::new(upstream, Arc::new(NoSecrets));
    assert!(matches!(
        manager.call(b"{}").await,
        Err(LocalStdioError::ExecutableDigestMismatch)
    ));
}
