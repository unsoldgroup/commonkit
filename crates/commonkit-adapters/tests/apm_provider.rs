#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, DesiredStateProvider, ExactProviderVersion,
    FilesystemIntent, NormalizedManagedPath, ProviderCapability, ProviderContext,
    ProviderWorkspace, redacted_apm_diagnostic_summary,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn documented_apm_mcp_output_becomes_provenance_bound_provider_capability() {
    let root = fixture("mcp-capability");
    let executable = root.join("apm");
    write_executable(
        &executable,
        r##"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\n'; exit 0; fi
if [ "$1" = "compile" ]; then
  mkdir -p .claude .codex
  printf 'context\n' > .claude/CLAUDE.md
  printf '{"mcpServers":{"docs":{"type":"streamable-http","url":"https://docs.example/mcp","headers":{"Authorization":"${env:DOCS_TOKEN}"}}}}' > .mcp.json
fi
if [ "$1" = "audit" ]; then printf '{"ok":true}\n'; fi
"##,
    );
    let provider = configured(&root, executable);
    fs::write(
        root.join("apm.yml"),
        r#"name: test
version: 1.0.0
dependencies:
  mcp:
    - name: docs
      registry: false
      transport: streamable-http
      url: https://docs.example/mcp
      headers:
        Authorization: ${env:DOCS_TOKEN}
"#,
    )
    .unwrap();
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let state = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap();
    assert_eq!(state.capabilities.len(), 1);
    assert!(matches!(&state.capabilities[0].capability,
        ProviderCapability::McpStreamableHttp { id, url, .. }
        if id == "docs" && url == "https://docs.example/mcp"));
    assert_eq!(
        state.capabilities[0].provenance.source,
        "apm.yml:dependencies.mcp[docs] -> .mcp.json:mcpServers.docs"
    );
    state.verify().unwrap();
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn ambiguous_or_conflicting_apm_mcp_output_fails_closed() {
    let root = fixture("mcp-conflict");
    let executable = root.join("apm");
    write_executable(
        &executable,
        r##"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\n'; exit 0; fi
if [ "$1" = "compile" ]; then
  mkdir -p .claude .codex
  printf 'context\n' > .claude/CLAUDE.md
  printf '{"mcpServers":{"docs":{"type":"streamable-http","url":"https://other.example/mcp"}}}' > .mcp.json
fi
if [ "$1" = "audit" ]; then printf '{"ok":true}\n'; fi
"##,
    );
    let provider = configured(&root, executable);
    fs::write(
        root.join("apm.yml"),
        r#"name: test
version: 1.0.0
dependencies:
  mcp:
    - name: docs
      registry: false
      transport: streamable-http
      url: https://docs.example/mcp
"#,
    )
    .unwrap();
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();
    assert!(error.contains("MCP") && error.contains("docs"), "{error}");
}

fn fixture(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "commonkit-apm-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn context() -> ProviderContext {
    ProviderContext {
        target_id: StableId::parse("local").unwrap(),
        platform: "macos".into(),
        architecture: "aarch64".into(),
        policy_digest: digest('f'),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::new(),
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn configured(root: &Path, executable: PathBuf) -> ApmProvider {
    configured_with_bound_source(root, executable, None)
}

fn configured_with_bound_source(
    root: &Path,
    executable: PathBuf,
    bound_source: Option<PathBuf>,
) -> ApmProvider {
    let manifest = root.join("apm.yml");
    let lockfile = root.join("apm.lock.yaml");
    let policy = root.join("apm-policy.yml");
    fs::write(&manifest, "packages: []\n").unwrap();
    fs::write(&lockfile, "lockfileVersion: 1\npackages: []\n").unwrap();
    fs::write(&policy, "allowedSources: [local]\n").unwrap();
    ApmProvider::new(ApmProviderConfig {
        executable,
        git_executable: None,
        version: ExactProviderVersion::parse("0.25.0").unwrap(),
        manifest,
        lockfile,
        policy,
        targets: vec!["claude".into(), "codex".into()],
        managed_root: NormalizedManagedPath::parse("home").unwrap(),
        bound_source,
    })
    .unwrap()
}

#[test]
fn invalid_inputs_fail_before_provider_execution() {
    let root = fixture("preflight");
    let marker = root.join("executed");
    let executable = root.join("apm");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let provider = configured(&root, executable);
    fs::remove_file(root.join("apm.lock.yaml")).unwrap();

    let error = provider.inspect_inputs(&context()).unwrap_err().to_string();
    assert!(error.contains("lockfile"), "{error}");
    assert!(
        !marker.exists(),
        "provider executable ran before preflight passed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn materialization_uses_immutable_executable_and_input_snapshots() {
    let root = fixture("immutable-snapshots");
    let executable = root.join("apm");
    fs::create_dir_all(root.join(".apm/instructions")).unwrap();
    fs::write(root.join(".apm/instructions/base.md"), "original-source\n").unwrap();
    let manifest = root.join("apm.yml");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  if [ -f "$HOME/apm.yml" ]; then
    printf '#!/bin/sh\nexit 93\n' > "$HOME/replacement-executable"
    mv -f "$HOME/replacement-executable" "$0" 2>/dev/null || :
    printf 'packages: [provider-replaced]\n' > "$HOME/replacement-manifest"
    snapshot_dir=${0%/*}
    mv -f "$HOME/replacement-manifest" "$snapshot_dir/apm.yml" 2>/dev/null || :
    touch "$HOME/race-ready"
    while [ ! -f "$HOME/race-go" ]; do sleep 0.01; done
  fi
  printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\n'
  exit 0
fi
if [ "$1" = "compile" ]; then
  mkdir -p .claude
  cat apm.yml .apm/instructions/base.md > .claude/CLAUDE.md
fi
if [ "$1" = "audit" ]; then printf '{"ok":true}\n'; fi
"#,
    );
    let provider = configured(&root, executable);
    fs::write(&manifest, "packages: [original]\n").unwrap();
    let approved = provider.inspect_inputs(&context()).unwrap();
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let race_root = root.clone();
    let race_stage = stage.clone();
    let racer = std::thread::spawn(move || {
        for _ in 0..1_000 {
            if race_stage.join("race-ready").exists() {
                fs::write(race_root.join("apm.yml"), "packages: [swapped]\n").unwrap();
                fs::write(
                    race_root.join(".apm/instructions/base.md"),
                    "swapped-source\n",
                )
                .unwrap();
                write_executable(&race_root.join("apm"), "#!/bin/sh\nexit 91\n");
                fs::write(race_stage.join("race-go"), b"").unwrap();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("provider did not reach race point");
    });

    let state = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap();
    racer.join().unwrap();

    assert_eq!(state.inputs, approved);
    let content = state
        .resources
        .iter()
        .find_map(|resource| match resource.intent.filesystem() {
            Some(FilesystemIntent::File { path, content, .. })
                if path.as_str() == "home/.claude/CLAUDE.md" =>
            {
                Some(artifacts.load(content).unwrap())
            }
            _ => None,
        })
        .expect("snapshotted output");
    assert_eq!(content, b"packages: [original]\noriginal-source\n");
}

#[test]
fn executable_replacement_invalidates_provider_before_launch() {
    let root = fixture("executable-replacement");
    let marker = root.join("executed");
    let executable = root.join("apm");
    write_executable(
        &executable,
        "#!/bin/sh\nprintf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\\n'\n",
    );
    let provider = configured(&root, executable.clone());
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("changed after provider initialization"),
        "{error}"
    );
    assert!(!marker.exists(), "replacement provider executable ran");
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn failed_provider_output_stays_private_and_exported_diagnostics_are_bounded_and_redacted() {
    let root = fixture("private-diagnostics");
    let executable = root.join("apm");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\n'; exit 0; fi
if [ "$1" = "install" ]; then printf 'ordinary failure detail\ntoken=super-secret-value\n' >&2; exit 1; fi
"#,
    );
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    let live = root.join("live");
    fs::create_dir_all(&stage).unwrap();
    fs::create_dir_all(&live).unwrap();
    let workspace = ProviderWorkspace::open(&stage, std::slice::from_ref(&live)).unwrap();
    let scratch = workspace.scratch_root().to_path_buf();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("super-secret-value"));
    let raw = fs::read_to_string(scratch.join("apm-install.stderr")).unwrap();
    assert!(raw.contains("super-secret-value"));
    assert_eq!(
        fs::metadata(scratch.join("apm-install.stderr"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let summary = redacted_apm_diagnostic_summary(&scratch, "install").unwrap();
    assert!(summary.contains("ordinary failure detail"));
    assert!(summary.contains("[REDACTED SENSITIVE LINE]"));
    assert!(!summary.contains("super-secret-value"));
    assert!(redacted_apm_diagnostic_summary(&scratch, "../outside").is_err());
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn pinned_apm_materializes_claude_and_codex_outside_the_live_target() {
    let root = fixture("materialize");
    let log = root.join("stage/argv.log");
    let executable = root.join("apm");
    write_executable(
        &executable,
        r##"#!/bin/sh
printf '%s\n' "$*" >> "$PWD/argv.log"
if [ "$1" = "--version" ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\n'; exit 0; fi
if [ "$1" = "compile" ]; then
  mkdir -p .claude .codex
  printf 'claude context\n' > .claude/CLAUDE.md
  printf 'codex context\n' > .codex/AGENTS.md
  printf 'root codex context\n' > AGENTS.md
fi
if [ "$1" = "audit" ]; then printf '{"ok":true}\n'; fi
"##,
    );
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    let live = root.join("live");
    let artifacts = root.join("artifacts");
    fs::create_dir_all(&stage).unwrap();
    fs::create_dir_all(&live).unwrap();
    let workspace = ProviderWorkspace::open(&stage, std::slice::from_ref(&live)).unwrap();
    let artifacts = ArtifactStore::open(&artifacts).unwrap();

    let inputs = provider.inspect_inputs(&context()).unwrap();
    assert_eq!(inputs.provider_version.as_str(), "0.25.0");
    assert!(inputs.input_digests.contains_key("manifest"));
    assert!(inputs.input_digests.contains_key("lockfile"));
    assert!(inputs.input_digests.contains_key("packagePolicy"));

    let state = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap();
    state.verify().unwrap();
    assert_eq!(state.resources.len(), 3);
    assert!(state.resources.iter().any(|resource| {
        resource
            .intent
            .filesystem()
            .is_some_and(|intent| intent.path().as_str() == "home/AGENTS.md")
    }));
    assert!(state.resources.iter().all(|resource| {
        resource.provenance.provider_id.as_str() == "apm"
            && resource.provenance.provider_version == "0.25.0"
            && resource.provenance.input_digest == *inputs.digest()
            && matches!(
                resource.intent.filesystem(),
                Some(FilesystemIntent::File { .. })
            )
    }));
    assert!(!live.join(".claude/CLAUDE.md").exists());
    assert!(!live.join(".codex/AGENTS.md").exists());

    let second_stage = root.join("second-stage");
    fs::create_dir_all(&second_stage).unwrap();
    let second_workspace =
        ProviderWorkspace::open(&second_stage, std::slice::from_ref(&live)).unwrap();
    let repeated = provider
        .materialize(&context(), &second_workspace, &artifacts)
        .unwrap();
    assert_eq!(state.digest, repeated.digest);
    assert_eq!(state.resources, repeated.resources);

    let argv = fs::read_to_string(log).unwrap();
    assert!(argv.contains("--version"), "{argv}");
    assert!(
        argv.contains("install --frozen --target claude,codex"),
        "{argv}"
    );
    assert!(argv.contains("compile --target claude,codex"), "{argv}");
    assert!(
        argv.contains(&format!(
            "audit --ci --policy {}/apm-policy.yml --no-fail-fast --format json",
            stage.canonicalize().unwrap().display()
        )),
        "{argv}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn provider_process_cannot_write_outside_its_isolated_workspace() {
    let root = fixture("sandbox-write-escape");
    let outside = root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let marker = outside.join("escaped");
    let executable = root.join("malicious-apm");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\\n'; exit 0; fi\nprintf escaped > '{}'\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();

    assert!(!marker.exists(), "provider escaped its writable workspace");
    assert!(
        error.contains("provider sandbox") || error.contains("APM command"),
        "unexpected diagnostic: {error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_process_cannot_open_network_connections() {
    if !Path::new("/usr/bin/curl").is_file() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let root = fixture("sandbox-network-escape");
    let executable = root.join("malicious-apm");
    write_executable(
        &executable,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'Agent Package Manager (APM) CLI version 0.25.0 (fixture)\\n'; exit 0; fi\n/usr/bin/curl --max-time 1 --silent http://127.0.0.1:{port}/ > \"$PWD/network-response\"\nexit 1\n"
        ),
    );
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let _ = provider.materialize(&context(), &workspace, &artifacts);

    assert!(
        listener.accept().is_err(),
        "provider opened a network connection outside its sandbox"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn version_mismatch_is_actionable_and_stops_before_compilation() {
    let root = fixture("version");
    let log = root.join("stage/argv.log");
    let executable = root.join("apm");
    write_executable(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$PWD/argv.log\"\nprintf 'Agent Package Manager (APM) CLI version 0.26.0 (fixture)\\n'\n",
    );
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();

    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected APM 0.25.0"), "{error}");
    assert!(error.contains("found 0.26.0"), "{error}");
    assert_eq!(fs::read_to_string(log).unwrap(), "--version\n");

    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "external providers fail closed on macOS"
)]
fn version_check_rejects_incidental_matching_numbers() {
    let root = fixture("version-format");
    let executable = root.join("apm");
    write_executable(&executable, "#!/bin/sh\nprintf 'wrapper build 0.25.0\\n'\n");
    let provider = configured(&root, executable);
    let stage = root.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let workspace = ProviderWorkspace::open(&stage, &[]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let error = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unrecognized version"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_sources_are_digest_bound_and_symlinks_fail_closed() {
    let root = fixture("sources");
    let executable = root.join("apm");
    write_executable(
        &executable,
        "#!/bin/sh\nprintf 'Agent Package Manager (APM) CLI version 0.25.0 (test)\\n'\n",
    );
    fs::create_dir_all(root.join(".apm/instructions")).unwrap();
    fs::write(
        root.join(".apm/instructions/base.instructions.md"),
        "first\n",
    )
    .unwrap();
    let provider = configured(&root, executable);
    let first = provider.inspect_inputs(&context()).unwrap();
    fs::write(
        root.join(".apm/instructions/base.instructions.md"),
        "second\n",
    )
    .unwrap();
    let second = provider.inspect_inputs(&context()).unwrap();
    assert_ne!(first.digest(), second.digest());
    assert_ne!(
        first.input_digests["projectSources"],
        second.input_digests["projectSources"]
    );
    let promoted = root.join("promoted-SKILL.md");
    fs::write(&promoted, "candidate one\n").unwrap();
    let bound = configured_with_bound_source(&root, root.join("apm"), Some(promoted.clone()));
    let bound_first = bound.inspect_inputs(&context()).unwrap();
    assert!(bound_first.input_digests.contains_key("promotedSource"));
    fs::write(&promoted, "candidate two\n").unwrap();
    let bound_second = bound.inspect_inputs(&context()).unwrap();
    assert_ne!(bound_first.digest(), bound_second.digest());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            "base.instructions.md",
            root.join(".apm/instructions/linked.md"),
        )
        .unwrap();
        let error = provider.inspect_inputs(&context()).unwrap_err().to_string();
        assert!(error.contains("symlink"), "{error}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_apm_025_release_materializes_only_in_disposable_staging_when_enabled() {
    let Some(executable) = std::env::var_os("COMMONKIT_APM_025_BIN").map(PathBuf::from) else {
        eprintln!("skipped: set COMMONKIT_APM_025_BIN to the checksum-verified APM 0.25.0 binary");
        return;
    };
    let root = fixture("real-release");
    fs::create_dir_all(root.join(".apm/instructions")).unwrap();
    fs::write(root.join("apm.yml"), "name: commonkit-real-spike\nversion: 1.0.0\ntargets: [claude, codex]\nincludes:\n  - .apm/instructions/\ndependencies:\n  apm: []\n  mcp: []\n").unwrap();
    fs::write(root.join("apm.lock.yaml"), "lockfile_version: '1'\ngenerated_at: '2026-07-18T00:00:00+00:00'\napm_version: 0.25.0\ndependencies: []\ndeployments: []\n").unwrap();
    fs::write(root.join("apm-policy.yml"), "version: 1\n").unwrap();
    fs::write(root.join(".apm/instructions/base.instructions.md"), "---\ndescription: CommonKit compatibility fixture\napplyTo: \"**\"\n---\n# Fixture\n\nRemain in staging.\n").unwrap();
    let live = root.join("live");
    let stage = root.join("stage");
    fs::create_dir_all(&live).unwrap();
    fs::create_dir_all(&stage).unwrap();
    let provider = ApmProvider::new(ApmProviderConfig {
        executable,
        git_executable: None,
        version: ExactProviderVersion::parse("0.25.0").unwrap(),
        manifest: root.join("apm.yml"),
        lockfile: root.join("apm.lock.yaml"),
        policy: root.join("apm-policy.yml"),
        targets: vec!["claude".into(), "codex".into()],
        managed_root: NormalizedManagedPath::parse("home").unwrap(),
        bound_source: None,
    })
    .unwrap();
    let workspace = ProviderWorkspace::open(&stage, std::slice::from_ref(&live)).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let state = provider
        .materialize(&context(), &workspace, &artifacts)
        .unwrap();
    state.verify().unwrap();
    assert!(state.resources.iter().any(|resource| {
        resource
            .intent
            .filesystem()
            .is_some_and(|intent| intent.path().as_str() == "home/AGENTS.md")
    }));
    assert!(state.resources.iter().any(|resource| {
        resource
            .intent
            .filesystem()
            .is_some_and(|intent| intent.path().as_str().starts_with("home/.claude/"))
    }));
    assert_eq!(
        fs::read_dir(&live).unwrap().count(),
        0,
        "APM touched the live target"
    );
    fs::remove_dir_all(root).unwrap();
}
