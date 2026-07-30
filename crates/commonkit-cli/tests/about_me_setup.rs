use commonkit_cli::about_me_setup::{SetupAnswers, SetupRequest, setup_profile};
use commonkit_about_me::{ProfileStore, ScopedView};
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn approved_interview_preserves_config_and_creates_a_private_searchable_profile() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    let state = root.path().join("state");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("headless.json"),
        br#"{"sync":{"targetId":"local"},"custom":{"preserved":true}}"#,
    )
    .unwrap();

    let outcome = setup_profile(SetupRequest {
        config_directory: config.clone(),
        state_directory: state.clone(),
        loadout_id: "personal".into(),
        project_id: "commonkit".into(),
        agent_id: "commonkit-agent".into(),
        answers: SetupAnswers {
            name: "Al".into(),
            explanation_style: "Use plain language and keep jargon low.".into(),
            decision_style: "Recommend one option first.".into(),
            tools: "Rust and TypeScript".into(),
            constraints: "Ask before changing product direction.".into(),
            never_assume: "Do not assume deep technical vocabulary.".into(),
        },
        approved: true,
    })
    .unwrap();

    let config_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.join("headless.json")).unwrap()).unwrap();
    assert_eq!(config_json["custom"]["preserved"], true);
    assert_eq!(config_json["aboutMe"]["loadoutId"], "personal");
    assert_eq!(
        config_json["aboutMe"]["keyReference"],
        format!("file://{}", outcome.key_path.display())
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&outcome.key_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    let key = std::fs::read(&outcome.key_path).unwrap();
    let mut store = ProfileStore::open(&outcome.database_path, &key).unwrap();
    let view = ScopedView {
        loadout_id: "personal".into(),
        project_id: "commonkit".into(),
    };
    assert_eq!(store.search(&view, "plain language", 5).unwrap().len(), 1);
    let raw = std::fs::read(&outcome.database_path).unwrap();
    assert!(!raw.windows("plain language".len()).any(|value| value == b"plain language"));
}

#[test]
fn declining_review_does_not_write_a_key_config_or_profile() {
    let root = tempfile::tempdir().unwrap();
    let request = SetupRequest {
        config_directory: root.path().join("config"),
        state_directory: root.path().join("state"),
        loadout_id: "personal".into(),
        project_id: "commonkit".into(),
        agent_id: "commonkit-agent".into(),
        answers: SetupAnswers::default(),
        approved: false,
    };

    assert!(setup_profile(request).is_err());
    assert!(!root.path().join("config/headless.json").exists());
    assert!(!root.path().join("config/about-me.key").exists());
    assert!(!root.path().join("state/about-me.sqlite").exists());
}

#[test]
fn setup_command_asks_plain_language_questions_and_saves_only_after_yes() {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    command
        .args([
            "about-me",
            "setup",
            "--loadout",
            "personal",
            "--project",
            "commonkit",
        ])
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"Al\nUse plain language.\nRecommend one option.\nRust\nAsk before broad changes.\nTechnical jargon.\nyes\n",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("What should I call you?"));
    assert!(text.contains("Nothing is saved until you approve"));
    assert!(text.contains("Profile saved"));
}
