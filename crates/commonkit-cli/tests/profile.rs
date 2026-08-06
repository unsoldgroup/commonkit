use std::process::Command;

fn isolated_command(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    command
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("local-appdata"));
    command
}

#[test]
fn profile_encryption_requires_confirmation_before_reading_answers() {
    let temporary = tempfile::tempdir().unwrap();
    let output = isolated_command(temporary.path())
        .args([
            "profile",
            "encrypt",
            "--answers",
            "does-not-exist.json",
            "--profile-id",
            "profile-1",
            "--revision-id",
            "revision-1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("confirmation_required"));
}

#[test]
fn confirmed_profile_is_staged_as_ciphertext_without_plaintext_output() {
    let temporary = tempfile::tempdir().unwrap();
    let answers = temporary.path().join("answers.json");
    let secret = "private profile value that must never be staged";
    std::fs::write(
        &answers,
        format!(r#"{{"identity.display_name":"{secret}"}}"#),
    )
    .unwrap();
    let recipients = (0..3)
        .map(|_| age::x25519::Identity::generate().to_public().to_string())
        .collect::<Vec<_>>();
    let mut command = isolated_command(temporary.path());
    command.args([
        "profile",
        "encrypt",
        "--answers",
        answers.to_str().unwrap(),
        "--profile-id",
        "profile-1",
        "--revision-id",
        "revision-1",
        "--confirmed",
    ]);
    for recipient in &recipients {
        command.args(["--recipient", recipient]);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));

    let staged = temporary
        .path()
        .join("data/state/personal-context/staged/revision-1.json");
    let bytes = std::fs::read(staged).unwrap();
    assert!(
        !bytes
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
    );
}
