#![cfg(unix)]
use commonkit_execd::secrets::{self, SecretsError};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn write_secrets(directory: &Path, contents: &str, mode: u32) -> PathBuf {
    let path = directory.join("secrets.json");
    std::fs::write(&path, contents).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn owner_only_secret_file_is_loaded() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_secrets(
        directory.path(),
        r#"{"env://GITHUB_STATUS_TOKEN":"token-value"}"#,
        0o600,
    );

    let resolved = secrets::load(&path).unwrap();

    assert_eq!(
        resolved
            .get("env://GITHUB_STATUS_TOKEN")
            .map(String::as_str),
        Some("token-value")
    );
}

#[test]
fn secret_file_readable_by_others_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_secrets(directory.path(), r#"{"env://TOKEN":"value"}"#, 0o644);

    let error = secrets::load(&path).unwrap_err();

    assert!(matches!(error, SecretsError::PermissionsTooOpen(0o644)));
}

#[test]
fn group_readable_secret_file_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_secrets(directory.path(), r#"{"env://TOKEN":"value"}"#, 0o640);

    assert!(matches!(
        secrets::load(&path).unwrap_err(),
        SecretsError::PermissionsTooOpen(_)
    ));
}

#[test]
fn references_outside_the_env_scheme_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_secrets(directory.path(), r#"{"file:///etc/token":"value"}"#, 0o600);

    assert!(matches!(
        secrets::load(&path).unwrap_err(),
        SecretsError::UnsupportedReference(_)
    ));
}

#[test]
fn reserved_environment_names_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    for reference in ["env://PATH", "env://HOME", "env://COMMONKIT_EXECD_TOKEN"] {
        let path = write_secrets(
            directory.path(),
            &format!(r#"{{"{reference}":"value"}}"#),
            0o600,
        );
        assert!(
            matches!(
                secrets::load(&path).unwrap_err(),
                SecretsError::UnsupportedReference(_)
            ),
            "{reference} was accepted"
        );
    }
}

/// A malformed file must not echo its contents, which are secret values.
#[test]
fn malformed_secret_file_error_does_not_quote_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_secrets(directory.path(), r#"{"env://TOKEN": 12345}"#, 0o600);

    let error = secrets::load(&path).unwrap_err();

    assert!(matches!(error, SecretsError::Malformed));
    assert!(!error.to_string().contains("12345"));
}

#[test]
fn a_missing_secret_file_is_an_error_rather_than_an_empty_map() {
    let directory = tempfile::tempdir().unwrap();

    assert!(matches!(
        secrets::load(&directory.path().join("absent.json")).unwrap_err(),
        SecretsError::Io(_)
    ));
}
