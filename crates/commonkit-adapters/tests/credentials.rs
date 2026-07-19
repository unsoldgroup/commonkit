use std::collections::BTreeMap;
use std::fs;

use commonkit_adapters::{
    BwsCommandError, BwsCommandRunner, BwsCredentialResolver, CredentialReadiness,
    CredentialReadinessInspector, CredentialReference, CredentialResolver, FakeCredentialResolver,
    LocalCredentialReadinessInspector, PlatformKeychain, PlatformKeychainCredentialResolver,
    PlatformSecretCommandError, PlatformSecretCommandRunner, WindowsCredentialManagerResolver,
    WindowsCredentialReader,
};
#[cfg(unix)]
use commonkit_adapters::{LocalSensitiveFileStore, NormalizedManagedPath};

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

#[derive(Default)]
struct RecordingWindowsReader {
    targets: Vec<String>,
}

impl WindowsCredentialReader for RecordingWindowsReader {
    fn read_generic(
        &mut self,
        target: &str,
    ) -> Result<Vec<u8>, commonkit_adapters::CredentialResolveError> {
        self.targets.push(target.into());
        Ok(b"windows-native-secret".to_vec())
    }
}

#[test]
fn windows_credential_manager_resolution_is_native_and_apply_time_only() {
    let reference = CredentialReference::parse("keychain://commonkit/api-token").unwrap();
    let mut resolver = WindowsCredentialManagerResolver::new(RecordingWindowsReader::default());
    let value = resolver.resolve(&reference).unwrap();
    assert_eq!(value.expose_for_apply(), b"windows-native-secret");
    assert_eq!(format!("{value:?}"), "SecretValue(<redacted>)");
    assert_eq!(
        resolver.reader().targets,
        vec!["commonkit/commonkit/api-token"]
    );
}

#[cfg(windows)]
#[test]
fn native_windows_credential_manager_round_trip_is_target_scoped() {
    use commonkit_adapters::NativeWindowsCredentialReader;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_SESSION, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredWriteW,
    };

    let service = format!("ci-{}", std::process::id());
    let account = "round-trip";
    let target = format!("commonkit/{service}/{account}");
    let mut target_wide = std::ffi::OsStr::new(&target)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut username = "commonkit-ci"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut blob = b"native-windows-secret".to_vec();
    let credential = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: target_wide.as_mut_ptr(),
        Comment: std::ptr::null_mut(),
        LastWritten: FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        },
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_SESSION,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: username.as_mut_ptr(),
    };
    assert_ne!(unsafe { CredWriteW(&credential, 0) }, 0);
    struct Cleanup(Vec<u16>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe { CredDeleteW(self.0.as_ptr(), CRED_TYPE_GENERIC, 0) };
        }
    }
    let _cleanup = Cleanup(target_wide.clone());
    let reference = CredentialReference::parse(format!("keychain://{service}/{account}")).unwrap();
    let value = WindowsCredentialManagerResolver::new(NativeWindowsCredentialReader)
        .resolve(&reference)
        .unwrap();
    assert_eq!(value.expose_for_apply(), b"native-windows-secret");
}

#[derive(Default)]
struct RecordingPlatformSecretRunner {
    calls: Vec<(String, Vec<String>)>,
}

impl PlatformSecretCommandRunner for RecordingPlatformSecretRunner {
    fn run(
        &mut self,
        executable: &str,
        args: &[String],
    ) -> Result<Vec<u8>, PlatformSecretCommandError> {
        self.calls.push((executable.to_owned(), args.to_vec()));
        Ok(b"native-secret\n".to_vec())
    }
}

#[test]
fn platform_keychains_use_fixed_arguments_and_expose_values_only_to_apply() {
    let reference = CredentialReference::parse("keychain://commonkit/api-token").unwrap();

    let mut mac = PlatformKeychainCredentialResolver::new(
        PlatformKeychain::MacOs,
        RecordingPlatformSecretRunner::default(),
    );
    let value = mac.resolve(&reference).unwrap();
    assert_eq!(value.expose_for_apply(), b"native-secret");
    assert_eq!(
        mac.runner().calls,
        vec![(
            "/usr/bin/security".to_owned(),
            vec![
                "find-generic-password".to_owned(),
                "-s".to_owned(),
                "commonkit".to_owned(),
                "-a".to_owned(),
                "api-token".to_owned(),
                "-w".to_owned(),
            ],
        )]
    );

    let mut linux = PlatformKeychainCredentialResolver::new(
        PlatformKeychain::LinuxSecretService,
        RecordingPlatformSecretRunner::default(),
    );
    assert_eq!(
        linux.resolve(&reference).unwrap().expose_for_apply(),
        b"native-secret"
    );
    assert_eq!(
        linux.runner().calls[0],
        (
            "/usr/bin/secret-tool".to_owned(),
            vec![
                "lookup".to_owned(),
                "service".to_owned(),
                "commonkit".to_owned(),
                "account".to_owned(),
                "api-token".to_owned(),
            ]
        )
    );
}

#[test]
fn platform_keychain_resolver_rejects_other_reference_schemes() {
    let reference = CredentialReference::parse("env://COMMONKIT_TOKEN").unwrap();
    let mut resolver = PlatformKeychainCredentialResolver::new(
        PlatformKeychain::MacOs,
        RecordingPlatformSecretRunner::default(),
    );
    assert_eq!(
        resolver.resolve(&reference).unwrap_err(),
        commonkit_adapters::CredentialResolveError::UnsupportedReference
    );
    assert!(resolver.runner().calls.is_empty());
}

#[cfg(not(windows))]
#[test]
fn windows_keychain_selection_never_falls_back_to_a_process_command() {
    let reference = CredentialReference::parse("keychain://commonkit/api-token").unwrap();
    let mut resolver = PlatformKeychainCredentialResolver::new(
        PlatformKeychain::WindowsCredentialManager,
        RecordingPlatformSecretRunner::default(),
    );
    assert_eq!(
        resolver.resolve(&reference).unwrap_err(),
        commonkit_adapters::CredentialResolveError::UnsupportedReference
    );
    assert!(resolver.runner().calls.is_empty());
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
    let file = CredentialReference::from_file_path(&existing).unwrap();
    let missing = CredentialReference::from_file_path(root.join("missing")).unwrap();
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
