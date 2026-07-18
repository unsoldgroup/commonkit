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
        platform: "macos".into(),
        architecture: "aarch64".into(),
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
    assert!(!apply.contains(&fixture.live.display().to_string()));

    let repeated = provider
        .materialize(&context(), &fixture.workspace(), &fixture.artifacts())
        .unwrap();
    assert_eq!(state.digest, repeated.digest);
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
    live: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = unique_temp_dir(label);
        let source = root.join("source");
        let config = root.join("chezmoi.toml");
        let executable = root.join("fake-chezmoi");
        let calls = root.join("calls");
        let live = root.join("live");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(root.join("stage")).unwrap();
        fs::create_dir_all(&live).unwrap();
        fs::write(&config, "[data]\n").unwrap();
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' "$*" >> '{}'
  echo 'chezmoi version 2.70.4'
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
            live,
        }
    }

    fn provider(&self) -> ChezmoiProvider {
        ChezmoiProvider::new(
            &self.executable,
            &self.source,
            &self.config,
            self.root.join("stage/cache"),
            self.root.join("stage/state"),
            self.root.join("stage/working"),
        )
        .unwrap()
    }

    fn workspace(&self) -> ProviderWorkspace {
        ProviderWorkspace::open(self.root.join("stage"), &[self.live.clone()]).unwrap()
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
