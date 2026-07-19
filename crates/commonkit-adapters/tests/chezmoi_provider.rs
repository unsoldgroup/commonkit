#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

use commonkit_adapters::{
    ArtifactStore, ChezmoiProvider, DesiredStateProvider, FilesystemIntent, NormalizedManagedPath,
    ProviderContext, ProviderWorkspace,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn context() -> ProviderContext {
    ProviderContext {
        target_id: StableId::parse("laptop").unwrap(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        policy_digest: digest('a'),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::new(),
    }
}

#[test]
fn materializes_only_into_isolated_destination_with_fixed_flags() {
    let fixture = Fixture::new("isolated");
    fs::write(fixture.source.join("dot_gitconfig"), "[user]\nname = Al\n").unwrap();
    let provider = fixture.provider();
    let inputs = provider.inspect_inputs(&context()).unwrap();
    assert_eq!(inputs.provider_version.as_str(), "2.70.4");

    let state = provider
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap();
    assert_eq!(state.inputs.provider_version.as_str(), "2.70.4");
    assert!(state.unsupported.is_empty());
    assert!(state.resources.iter().any(|resource| matches!(
        &resource.intent,
        FilesystemIntent::File { path, .. } if path.as_str() == "home/.gitconfig"
    )));

    let calls = fs::read_to_string(&fixture.calls).unwrap();
    assert!(calls.lines().next().unwrap().contains("--version"));
    let apply = calls.lines().find(|line| line.contains(" apply")).unwrap();
    for required in [
        "--source",
        "--config",
        "--destination",
        "--cache",
        "--persistent-state",
        "--working-tree",
        "--mode=file",
        "--no-tty",
        "--no-pager",
        "--color=off",
        "--refresh-externals=never",
        "--exclude=scripts",
        "apply",
    ] {
        assert!(
            apply.contains(required),
            "missing fixed argument {required}: {apply}"
        );
    }
    assert!(!apply.contains("--os"), "unsupported --os flag: {apply}");
    assert!(
        !apply.contains("--arch"),
        "unsupported --arch flag: {apply}"
    );
    assert!(!apply.contains(&fixture.live.display().to_string()));

    let repeated = provider
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap();
    assert_eq!(state.digest, repeated.digest);
}

#[test]
fn rejects_materialization_for_a_different_target_platform_before_apply() {
    let fixture = Fixture::new("cross-platform-target");
    fs::write(
        fixture.source.join("dot_machine.tmpl"),
        "{{ .chezmoi.os }}\n",
    )
    .unwrap();
    let mut remote = context();
    remote.platform = if std::env::consts::OS == "linux" {
        "macos".into()
    } else {
        "linux".into()
    };

    let error = fixture
        .provider()
        .materialize(&remote, &fixture.workspace(), &fixture.artifacts())
        .expect_err("chezmoi cannot emulate another target platform")
        .to_string();

    assert!(error.contains("cannot emulate target platform"), "{error}");
    let calls = fs::read_to_string(&fixture.calls).unwrap_or_default();
    assert!(
        !calls.lines().any(|line| line.contains(" apply")),
        "{calls}"
    );
    assert_eq!(fs::read_dir(&fixture.live).unwrap().count(), 0);
}

#[test]
fn target_platform_facts_change_provider_inputs_not_controller_constants() {
    let fixture = Fixture::new("target-platform");
    let controller = fixture.provider().inspect_inputs(&context()).unwrap();
    let mut other_context = context();
    other_context.platform = if std::env::consts::OS == "linux" {
        "macos".into()
    } else {
        "linux".into()
    };
    other_context.architecture = if std::env::consts::ARCH == "x86_64" {
        "aarch64".into()
    } else {
        "x86_64".into()
    };
    let other_target = fixture.provider().inspect_inputs(&other_context).unwrap();

    assert_ne!(controller.input_set_digest, other_target.input_set_digest);
    assert_ne!(
        controller.input_digests["targetPlatform"],
        other_target.input_digests["targetPlatform"]
    );
}

#[test]
fn accepts_the_official_pinned_version_output_and_rejects_other_versions() {
    let fixture = Fixture::new("official-version");
    fs::write(
        &fixture.version_output,
        "chezmoi version v2.70.4, commit 64583685c5eb36e10670bad076d5406a08baf751, built at 2026-05-19T22:47:23Z, built by goreleaser\n",
    )
    .unwrap();
    fixture.provider().inspect_inputs(&context()).unwrap();

    fs::write(
        &fixture.version_output,
        "chezmoi version v2.70.40, commit malicious\n",
    )
    .unwrap();
    let error = fixture
        .provider()
        .inspect_inputs(&context())
        .unwrap_err()
        .to_string();
    assert!(error.contains("2.70.4 is required"), "{error}");
}

#[test]
fn rejects_unsafe_source_features_before_starting_a_process() {
    let cases = [
        ("run_before_setup.sh", "script"),
        (".chezmoiexternal.toml", "external"),
        ("modify_config", "modify"),
        ("create_token", "create"),
        ("exact_dot_config", "exact_directory"),
        ("encrypted.age", "encrypted_source"),
        (".chezmoiscripts", "script"),
        (".chezmoiexternals", "external"),
        ("dot_config.tmpl", "destination_dependent_template"),
    ];
    for (name, capability) in cases {
        let fixture = Fixture::new(capability);
        let content = if name.ends_with(".tmpl") {
            "{{ .chezmoi.destDir }}"
        } else {
            "x"
        };
        fs::write(fixture.source.join(name), content).unwrap();
        let error = fixture
            .provider()
            .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(name),
            "diagnostic lacked source entry: {error}"
        );
        assert!(
            error.contains(capability),
            "diagnostic lacked capability: {error}"
        );
        assert!(
            !fixture.calls.exists(),
            "provider process started for {name}"
        );
    }
}

#[test]
#[ignore = "requires COMMONKIT_CHEZMOI_2_70_4 pointing to the official pinned binary"]
fn real_chezmoi_release_materializes_supported_fixtures_deterministically() {
    let executable = std::env::var_os("COMMONKIT_CHEZMOI_2_70_4")
        .expect("set COMMONKIT_CHEZMOI_2_70_4 to the official chezmoi 2.70.4 binary");
    let fixture = Fixture::new("real-release");
    fs::write(fixture.source.join("dot_regular"), "regular\n").unwrap();
    fs::write(fixture.source.join("executable_dot_tool"), "#!/bin/sh\n").unwrap();
    fs::write(fixture.source.join("private_dot_secret"), "private\n").unwrap();
    fs::write(fixture.source.join("readonly_dot_readonly"), "readonly\n").unwrap();
    fs::create_dir(fixture.source.join("dot_config")).unwrap();
    fs::write(fixture.source.join("dot_config/app"), "app\n").unwrap();
    fs::write(fixture.source.join("symlink_dot_link"), ".regular\n").unwrap();
    fs::write(
        fixture.source.join("dot_machine.tmpl"),
        "{{ .chezmoi.os }}-{{ .chezmoi.arch }}\n",
    )
    .unwrap();
    fs::write(fixture.source.join("dot_ignored"), "ignored\n").unwrap();
    fs::write(fixture.source.join(".chezmoiignore"), ".ignored\n").unwrap();

    let provider = fixture.provider_with_executable(PathBuf::from(executable));
    let first = provider
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap();
    let second = provider
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap();
    assert_eq!(first.digest, second.digest);
    let go_os = if std::env::consts::OS == "macos" {
        "darwin"
    } else {
        std::env::consts::OS
    };
    let go_arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        architecture => architecture,
    };
    let machine = first
        .resources
        .iter()
        .find_map(|resource| match &resource.intent {
            FilesystemIntent::File { path, content, .. } if path.as_str() == "home/.machine" => {
                Some(fixture.artifacts().load(content).unwrap())
            }
            _ => None,
        })
        .expect("machine-conditional output");
    assert_eq!(machine, format!("{go_os}-{go_arch}\n").as_bytes());
    assert!(first.resources.iter().any(|resource| matches!(
        &resource.intent,
        FilesystemIntent::Symlink { path, target, .. }
            if path.as_str() == "home/.link" && target.as_str() == ".regular"
    )));
    assert!(
        !first
            .resources
            .iter()
            .any(|resource| { resource.intent.path().as_str() == "home/.ignored" })
    );
    for (path, expected_mode) in [
        ("home/.regular", 0o644),
        ("home/.tool", 0o755),
        ("home/.secret", 0o600),
        ("home/.readonly", 0o444),
    ] {
        assert!(
            first.resources.iter().any(|resource| matches!(
                &resource.intent,
                FilesystemIntent::File { path: actual, mode: Some(mode), .. }
                    if actual.as_str() == path && mode.value() == expected_mode
            )),
            "missing {path} with mode {expected_mode:o}"
        );
    }
    assert!(first.resources.iter().any(|resource| matches!(
        &resource.intent,
        FilesystemIntent::Directory { path, exact: false, .. }
            if path.as_str() == "home/.config"
    )));
    assert_eq!(fs::read_dir(&fixture.live).unwrap().count(), 0);
}

#[test]
fn rejects_host_process_network_and_secret_template_functions_before_execution() {
    for token in [
        "env",
        "exec",
        "output",
        "httpResponse",
        "secret",
        "onepasswordRead",
        "stat",
    ] {
        let fixture = Fixture::new(token);
        let source = fixture.source.join("dot_config.tmpl");
        fs::write(&source, format!("{{{{ {token} \"value\" }}}}")).unwrap();
        let error = fixture
            .provider()
            .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
            .unwrap_err()
            .to_string();
        assert!(error.contains("dot_config.tmpl"));
        assert!(error.contains("unsafe_template_function"));
        assert!(!fixture.calls.exists());
    }
}

#[test]
fn rejects_config_hooks_before_execution() {
    let fixture = Fixture::new("config-hook");
    fs::write(
        &fixture.config,
        "[hooks.read-source-state.pre]\ncommand = \"steal-secrets\"\n",
    )
    .unwrap();

    let error = fixture
        .provider()
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap_err()
        .to_string();
    assert!(error.contains("config"), "{error}");
    assert!(error.contains("hook"), "{error}");
    assert!(!fixture.calls.exists());
}

#[test]
fn scans_regular_files_directories_and_safe_symlinks_from_staging() {
    let fixture = Fixture::new("scan");
    fs::write(fixture.source.join("dot_gitconfig"), "git").unwrap();
    fs::create_dir(fixture.source.join("dot_config")).unwrap();
    fs::write(fixture.source.join("dot_config/app"), "app").unwrap();
    symlink(".gitconfig", fixture.source.join("dot_gitconfig-link")).unwrap();

    let state = fixture
        .provider()
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap();
    assert!(
        state.resources.iter().any(|r| matches!(&r.intent,
        FilesystemIntent::Directory { path, exact: false, .. } if path.as_str() == "home/.config"))
    );
    assert!(state.resources.iter().any(|r| matches!(&r.intent,
        FilesystemIntent::Symlink { path, target, .. }
            if path.as_str() == "home/.gitconfig-link" && target.as_str() == ".gitconfig")));
}

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    config: PathBuf,
    executable: PathBuf,
    calls: PathBuf,
    version_output: PathBuf,
    live: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = unique_temp_dir(label);
        let source = root.join("source");
        let config = root.join("chezmoi.toml");
        let executable = root.join("fake-chezmoi");
        let calls = root.join("calls");
        let version_output = root.join("version-output");
        let live = root.join("live");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(root.join("stage")).unwrap();
        fs::create_dir_all(&live).unwrap();
        fs::write(&config, "[data]\n").unwrap();
        fs::write(&version_output, "chezmoi version v2.70.4\n").unwrap();
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' "$*" >> '{}'
  cat '{}'
  exit 0
fi
printf '%s\n' "$*" >> '{}'
dest=''
source=''
previous=''
for arg in "$@"; do
  if [ "$previous" = '--destination' ]; then dest="$arg"; fi
  if [ "$previous" = '--source' ]; then source="$arg"; fi
  previous="$arg"
done
mkdir -p "$dest"
cp -R "$source"/. "$dest"/
find "$dest" -name 'dot_*' | while read path; do
  parent=$(dirname "$path")
  base=$(basename "$path")
  mv "$path" "$parent/.${{base#dot_}}"
done
"#,
                calls.display(),
                version_output.display(),
                calls.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            source,
            config,
            executable,
            calls,
            version_output,
            live,
        }
    }

    fn provider(&self) -> ChezmoiProvider {
        self.provider_with_executable(self.executable.clone())
    }

    fn provider_with_executable(&self, executable: PathBuf) -> ChezmoiProvider {
        ChezmoiProvider::new(
            executable,
            &self.source,
            &self.config,
            self.root.join("stage/cache"),
            self.root.join("stage/state"),
            self.root.join("stage/working"),
        )
        .unwrap()
    }

    fn workspace(&self) -> ProviderWorkspace {
        ProviderWorkspace::open(self.root.join("stage"), std::slice::from_ref(&self.live)).unwrap()
    }

    fn artifacts(&self) -> ArtifactStore {
        ArtifactStore::open(self.root.join("artifacts")).unwrap()
    }
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("commonkit-chezmoi-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}
