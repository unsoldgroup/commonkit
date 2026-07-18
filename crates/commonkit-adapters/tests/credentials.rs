use std::collections::BTreeMap;
use std::fs;

use commonkit_adapters::{
    BwsCommandError, BwsCommandRunner, BwsCredentialResolver, CredentialReadiness,
    CredentialReadinessInspector, CredentialReference, CredentialResolver, FakeCredentialResolver,
    LocalCredentialReadinessInspector, LocalSensitiveFileStore, NormalizedManagedPath,
};

#[test]
fn accepts_only_strict_reference_uris_and_serializes_only_the_reference() {
    for value in [
        "env://COMMONKIT_TOKEN",
        "file:///var/run/secrets/commonkit-token",
        "keychain://commonkit/api-token",
        "bws://8f0f6cab-5e55-4a73-99e3-20c312706da1",
    ] {
        let reference = CredentialReference::parse(value).expect(value);
        assert_eq!(reference.as_str(), value);
        assert_eq!(
            serde_json::to_string(&reference).unwrap(),
            format!("\"{value}\"")
        );
    }

    for value in [
        "literal-secret",
        "secret://legacy",
        "env://",
        "env://NOT VALID",
        "env://name?fallback=secret",
        "file://relative/path",
        "file:///tmp/../secret",
        "keychain://service",
        "bws://token?project=x",
        "https://user:password@example.com",
    ] {
        let error = CredentialReference::parse(value).expect_err(value);
        assert!(!error.to_string().contains(value), "error leaked input");
    }
}

fn temporary_directory(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "commonkit-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn target_local_sensitive_files_are_private_and_never_follow_symlinks() {
    use std::os::unix::fs::{MetadataExt, symlink};

    let root = temporary_directory("credentials-files");
    let outside = temporary_directory("credentials-outside");
    let store = LocalSensitiveFileStore::open(&root).unwrap();
    let path = NormalizedManagedPath::parse("runtime/token").unwrap();
    let secret = commonkit_adapters::SecretValue::new(b"private-token".to_vec()).unwrap();
    store.write(&path, &secret).unwrap();

    assert_eq!(
        fs::metadata(root.join("runtime/token")).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(root.join("runtime")).unwrap().mode() & 0o777,
        0o700
    );

    symlink(outside.join("captured"), root.join("redirect")).unwrap();
    let redirect = NormalizedManagedPath::parse("redirect").unwrap();
    let error = store
        .write(&redirect, &secret)
        .expect_err("symlink rejected");
    assert_eq!(error.to_string(), "unsafe sensitive-file path");
    assert!(!outside.join("captured").exists());
}

struct StaticInspector;

impl CredentialReadinessInspector for StaticInspector {
    fn inspect(&self, reference: &CredentialReference) -> CredentialReadiness {
        if reference.scheme() == "env" {
            CredentialReadiness::Ready
        } else {
            CredentialReadiness::Unavailable
        }
    }
}

#[test]
fn readiness_is_safe_to_serialize_and_does_not_resolve_values() {
    let reference = CredentialReference::parse("env://COMMONKIT_TOKEN").unwrap();
    let readiness = StaticInspector.inspect(&reference);
    assert_eq!(readiness, CredentialReadiness::Ready);
    assert_eq!(serde_json::to_string(&readiness).unwrap(), "\"ready\"");
}

#[test]
fn local_readiness_uses_metadata_and_never_invokes_external_providers() {
    let root = temporary_directory("credential-readiness");
    let existing = root.join("token");
    fs::write(&existing, b"not-read-by-inspection").unwrap();
    let inspector = LocalCredentialReadinessInspector;
    let file = CredentialReference::parse(format!("file://{}", existing.display())).unwrap();
    let missing = CredentialReference::parse(format!("file://{}/missing", root.display())).unwrap();
    let bws = CredentialReference::parse("bws://opaque-id").unwrap();

    assert_eq!(inspector.inspect(&file), CredentialReadiness::Ready);
    assert_eq!(inspector.inspect(&missing), CredentialReadiness::Missing);
    assert_eq!(inspector.inspect(&bws), CredentialReadiness::Unavailable);
}

#[test]
fn resolved_values_are_apply_time_only_and_always_redacted() {
    let reference = CredentialReference::parse("env://COMMONKIT_TOKEN").unwrap();
    let mut resolver = FakeCredentialResolver::new(BTreeMap::from([(
        reference.clone(),
        b"super-secret-value".to_vec(),
    )]));
    let value = resolver.resolve(&reference).unwrap();
    assert_eq!(value.expose_for_apply(), b"super-secret-value");
    assert_eq!(format!("{value:?}"), "SecretValue(<redacted>)");
    assert_eq!(value.to_string(), "<redacted>");
}

#[derive(Default)]
struct RecordingBwsRunner {
    calls: Vec<Vec<String>>,
}

impl BwsCommandRunner for RecordingBwsRunner {
    fn run(&mut self, args: &[String]) -> Result<Vec<u8>, BwsCommandError> {
        self.calls.push(args.to_vec());
        Ok(br#"{"value":"resolved-by-bws"}"#.to_vec())
    }
}

#[test]
fn bws_resolution_uses_a_fixed_argv_without_a_shell_or_reference_leaks() {
    let reference =
        CredentialReference::parse("bws://8f0f6cab-5e55-4a73-99e3-20c312706da1").unwrap();
    let mut resolver = BwsCredentialResolver::new(RecordingBwsRunner::default());
    let value = resolver.resolve(&reference).unwrap();
    assert_eq!(value.expose_for_apply(), b"resolved-by-bws");
    assert_eq!(
        resolver.runner().calls,
        vec![vec![
            "secret".to_string(),
            "get".to_string(),
            "8f0f6cab-5e55-4a73-99e3-20c312706da1".to_string(),
        ]]
    );
}
