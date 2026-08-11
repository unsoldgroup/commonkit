#[cfg(unix)]
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
#[cfg(unix)]
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use commonkit_adapters::{
    AptCommandSpecV1, AptRepositoryConfigurationV1, AptResolutionBackend,
    AptResolutionSystemRequestV1, AptResolvedPackageV1, AptSourceAuthorityV1, ArtifactStore,
    ControlledPackageSourceV1, ManagerBindingV1, NodeResolutionBackend, NodeRuntimeHost,
    NodeRuntimeHostError, PackageDesiredIntent, PackageDiscoveryFetchRequestV1, PackageFetch,
    PackageFetchHopV1, PackageFetchRequestV1, PackageFetchResultV1, PackageResolutionCoordinator,
    PackageResolutionError, PackageSourceRegistry, PackageTargetV1,
    ProcessAptResolutionCommandRunner, ProcessNodeReleaseSignatureVerifier, ProcessNodeRuntimeHost,
};
#[cfg(unix)]
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SecurityPolicy, Sha256Digest, StableId,
};
#[cfg(unix)]
use futures_util::StreamExt;
#[cfg(unix)]
use reqwest::header::LOCATION;

const OPT_IN_ENV: &str = "COMMONKIT_NATIVE_PACKAGE_RESOLUTION";

#[test]
fn native_resolution_harness_requires_an_exact_opt_in() {
    assert!(!native_harness_enabled(None));
    assert!(!native_harness_enabled(Some(OsStr::new("0"))));
    assert!(!native_harness_enabled(Some(OsStr::new("true"))));
    assert!(native_harness_enabled(Some(OsStr::new("1"))));
}

#[test]
#[should_panic(expected = "set COMMONKIT_NATIVE_PACKAGE_RESOLUTION=1")]
fn explicitly_running_native_evidence_without_opt_in_fails() {
    require_native_harness_opt_in(None);
}

#[cfg(unix)]
#[test]
fn read_only_apt_plan_accepts_only_safe_install_modes_in_private_state() {
    let simulated = apt_install_command(&["--simulate"]);
    let downloaded = apt_install_command(&["--print-uris", "--download-only"]);

    assert!(validate_read_only_apt_commands(&[simulated]).is_ok());
    assert!(validate_read_only_apt_commands(&[downloaded]).is_ok());
}

#[cfg(unix)]
#[test]
fn production_apt_command_plan_matches_only_closed_read_only_shapes() {
    let source_id = stable_id("ubuntu-main");
    let authority = AptSourceAuthorityV1 {
        suite: "noble".into(),
        components: BTreeSet::from(["main".into()]),
        signing_authority: stable_id("ubuntu-archive-keyring"),
        signing_key_digest: fixture_digest('d'),
    };
    let registry = PackageSourceRegistry::builtin()
        .unwrap()
        .with_apt_source_authority(&source_id, authority.clone())
        .unwrap();
    let request = AptResolutionSystemRequestV1 {
        declaration: PackageDeclaration {
            id: stable_id("curl"),
            version: "8.5.0-2ubuntu10.6".into(),
            manager: PackageManager::Apt,
            source: source_id.clone(),
            selector: Some(PackageSelector::AptBinary {
                name: "curl".into(),
                architecture: Some("amd64".into()),
            }),
        },
        target: PackageTargetV1 {
            os: "linux".into(),
            os_version: "24.04".into(),
            distro_id: Some("ubuntu".into()),
            distro_version: Some("24.04".into()),
            codename: Some("noble".into()),
            arch: "amd64".into(),
            libc: Some("glibc".into()),
            manager_prefix: None,
        },
        manager: ManagerBindingV1 {
            manager: PackageManager::Apt,
            version: "apt:2.7.14;dpkg:1.22.6".into(),
            executable_digest: fixture_digest('a'),
            config_digest: fixture_digest('b'),
        },
        source_id: source_id.clone(),
        canonical_repository: "https://archive.ubuntu.com/ubuntu".into(),
        registry_definition_digest: registry.source_definition_digest(&source_id).unwrap(),
        source_authority: authority,
        repository: AptRepositoryConfigurationV1 {
            source_id,
            suite: "noble".into(),
            components: BTreeSet::from(["main".into()]),
            signed_by: PathBuf::from("/usr/share/keyrings/ubuntu-archive-keyring.gpg"),
            signing_authority: stable_id("ubuntu-archive-keyring"),
        },
    };

    let mut commands = ProcessAptResolutionCommandRunner::command_snapshot(&request).unwrap();
    commands.extend(
        ProcessAptResolutionCommandRunner::package_metadata_command_snapshot(&[
            AptResolvedPackageV1 {
                name: "curl".into(),
                version: "8.5.0-2ubuntu10.6".into(),
                architecture: "amd64".into(),
            },
            AptResolvedPackageV1 {
                name: "libc6".into(),
                version: "2.39-0ubuntu8.4".into(),
                architecture: "amd64".into(),
            },
        ])
        .unwrap(),
    );

    for (index, command) in commands.into_iter().enumerate() {
        assert_eq!(
            validate_read_only_apt_commands(&[command]),
            Ok(()),
            "production APT command {index}"
        );
    }
}

#[cfg(unix)]
#[test]
fn read_only_apt_plan_rejects_real_or_ambiguously_safe_install_commands() {
    let real_install = apt_install_command(&[]);
    let conflicting_modes = apt_install_command(&["--simulate", "--print-uris", "--download-only"]);
    let mut public_state = apt_install_command(&["--simulate"]);
    public_state
        .environment
        .insert("APT_CONFIG".into(), "/etc/apt/apt.conf".into());
    let mut build_dependencies = apt_install_command(&["--simulate"]);
    let action = build_dependencies
        .args
        .iter_mut()
        .find(|argument| argument.as_str() == "install")
        .unwrap();
    *action = "build-dep".into();
    let dpkg_install = AptCommandSpecV1 {
        executable: "/usr/bin/dpkg".into(),
        args: vec!["-i".into(), "<private>/curl.deb".into()],
        environment: BTreeMap::new(),
        network: false,
    };

    for unsafe_command in [
        real_install,
        conflicting_modes,
        public_state,
        build_dependencies,
        dpkg_install,
    ] {
        assert!(validate_read_only_apt_commands(&[unsafe_command]).is_err());
    }
}

#[cfg(unix)]
#[test]
fn read_only_apt_plan_rejects_late_duplicate_or_unrecognized_apt_overrides() {
    let mut host_state_override = apt_install_command(&["--simulate"]);
    host_state_override.args.extend([
        "-o".into(),
        "Dir::State::status=/var/lib/dpkg/status".into(),
    ]);
    let mut disables_simulation = apt_install_command(&["--simulate"]);
    insert_apt_option_before_action(&mut disables_simulation, "APT::Get::Simulate=false");
    let mut duplicate_security_option = apt_install_command(&["--simulate"]);
    insert_apt_option_before_action(
        &mut duplicate_security_option,
        "APT::Get::AllowUnauthenticated=false",
    );
    let mut hook_override = apt_install_command(&["--simulate"]);
    insert_apt_option_before_action(&mut hook_override, "DPkg::Pre-Invoke=/tmp/escape");
    let mut replaced_private_directory = apt_install_command(&["--simulate"]);
    *replaced_private_directory
        .args
        .iter_mut()
        .find(|argument| argument.as_str() == "Dir::State::lists=<private>/lists")
        .unwrap() = "Dir::State::lists=/var/lib/apt/lists".into();
    let mut host_package_cache = apt_install_command(&["--simulate"]);
    *host_package_cache
        .args
        .iter_mut()
        .find(|argument| argument.as_str() == "Dir::Cache::pkgcache=<private>/pkgcache.bin")
        .unwrap() = "Dir::Cache::pkgcache=/var/cache/apt/pkgcache.bin".into();
    let mut missing_security_option = apt_install_command(&["--simulate"]);
    let assignment = missing_security_option
        .args
        .iter()
        .position(|argument| argument == "Acquire::Check-Valid-Until=true")
        .unwrap();
    missing_security_option
        .args
        .drain(assignment - 1..=assignment);

    for unsafe_command in [
        host_state_override,
        disables_simulation,
        duplicate_security_option,
        hook_override,
        replaced_private_directory,
        host_package_cache,
        missing_security_option,
    ] {
        assert!(validate_read_only_apt_commands(&[unsafe_command]).is_err());
    }
}

#[cfg(unix)]
fn insert_apt_option_before_action(command: &mut AptCommandSpecV1, assignment: &str) {
    let action = command
        .args
        .iter()
        .position(|argument| argument == "--simulate")
        .expect("fixture has a simulation action");
    command
        .args
        .splice(action..action, ["-o".into(), assignment.into()]);
}

#[cfg(unix)]
#[test]
fn generated_nvm_negative_control_rejects_default_packages_with_exact_reason() {
    let root = tempfile::tempdir().unwrap();
    let shell = root.path().join("bash");
    let keyring = root.path().join("node-release-keyring.kbx");
    let target = PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "amd64".into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    };

    assert_generated_unsafe_nvm_control(
        root.path(),
        include_bytes!("fixtures/node/nvm-v0.40.6.sh"),
        &shell,
        &keyring,
        &target,
    );
}

#[cfg(unix)]
fn apt_install_command(safety_flags: &[&str]) -> AptCommandSpecV1 {
    let mut args = vec![
        "-o".into(),
        "Dir::Etc::sourcelist=<private>/sources.list".into(),
        "-o".into(),
        "Dir::Etc::sourceparts=-".into(),
        "-o".into(),
        "Dir::State::lists=<private>/lists".into(),
        "-o".into(),
        "Dir::Cache::archives=<private>/archives".into(),
        "-o".into(),
        "Dir::Cache::pkgcache=<private>/pkgcache.bin".into(),
        "-o".into(),
        "Dir::Cache::srcpkgcache=<private>/srcpkgcache.bin".into(),
        "-o".into(),
        "APT::Get::List-Cleanup=0".into(),
        "-o".into(),
        "Acquire::AllowInsecureRepositories=false".into(),
        "-o".into(),
        "Acquire::AllowWeakRepositories=false".into(),
        "-o".into(),
        "Acquire::AllowDowngradeToInsecureRepositories=false".into(),
        "-o".into(),
        "APT::Get::AllowUnauthenticated=false".into(),
        "-o".into(),
        "Acquire::Check-Valid-Until=true".into(),
        "-o".into(),
        "Acquire::Check-Date=true".into(),
        "-o".into(),
        "Acquire::By-Hash=force".into(),
        "-o".into(),
        "Acquire::http::AllowRedirect=false".into(),
        "-o".into(),
        "Acquire::https::AllowRedirect=false".into(),
        "-o".into(),
        "Acquire::http::Proxy=DIRECT".into(),
        "-o".into(),
        "Acquire::https::Proxy=DIRECT".into(),
        "-o".into(),
        "Acquire::Languages=none".into(),
        "-o".into(),
        "Dir::State::status=<private>/status".into(),
    ];
    if safety_flags == ["--print-uris", "--download-only"] {
        args.push("--quiet=2".into());
    }
    args.extend(safety_flags.iter().map(|flag| (*flag).to_owned()));
    args.extend(["--no-remove".into(), "--no-install-recommends".into()]);
    if safety_flags == ["--print-uris", "--download-only"] {
        args.push("--reinstall".into());
    }
    args.extend(["install".into(), "curl=1".into()]);
    AptCommandSpecV1 {
        executable: "/usr/bin/apt-get".into(),
        args,
        environment: BTreeMap::from([("APT_CONFIG".into(), "<private>/apt.conf".into())]),
        network: false,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
#[test]
fn native_linux_harness_remains_compile_checked_off_target() {
    let harness: fn() = run_linux_harness;
    assert_eq!(std::mem::size_of_val(&harness), std::mem::size_of::<fn()>());
}

#[test]
#[ignore = "requires explicit Linux package-resolution evidence opt-in"]
fn production_apt_and_node_resolvers_reopen_offline_without_target_mutation() {
    run_native_read_only_resolution_harness();
}

fn native_harness_enabled(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

fn require_native_harness_opt_in(value: Option<&OsStr>) {
    assert!(
        native_harness_enabled(value),
        "set COMMONKIT_NATIVE_PACKAGE_RESOLUTION=1 to run native package-resolution evidence"
    );
}

fn run_native_read_only_resolution_harness() {
    require_native_harness_opt_in(std::env::var_os(OPT_IN_ENV).as_deref());
    #[cfg(not(target_os = "linux"))]
    panic!("native package-resolution evidence requires Linux");
    #[cfg(target_os = "linux")]
    run_linux_harness();
}

#[cfg(unix)]
fn run_linux_harness() {
    let config = NativeHarnessConfig::from_environment();
    let root = redacted(tempfile::tempdir(), "private evidence workspace creation");
    let store = redacted(
        ArtifactStore::open(root.path().join("artifacts")),
        "private artifact store creation",
    );
    let mut fetch = redacted(NativeHttpsFetch::new(), "scoped HTTPS client creation");

    let apt_target = config.apt_target();
    let apt_repository = config.apt_repository();
    let apt_manager = redacted(
        ProcessAptResolutionCommandRunner::probe_manager_binding(
            &apt_target,
            &config.apt_repository_url,
            &apt_repository,
        ),
        "APT manager authority probe",
    );
    let apt_authority = redacted(
        ProcessAptResolutionCommandRunner::probe_source_authority(
            &apt_target,
            &config.apt_repository_url,
            &apt_repository,
        ),
        "APT source authority probe",
    );
    let apt_registry = redacted(
        PackageSourceRegistry::from_controlled_sources(vec![ControlledPackageSourceV1 {
            source_id: config.apt_source_id.clone(),
            manager: PackageManager::Apt,
            canonical_repository: config.apt_repository_url.clone(),
            approved_artifact_roots: BTreeSet::new(),
            apt_source_authority: Some(apt_authority.clone()),
        }]),
        "APT source registry construction",
    );
    let apt_declaration = PackageDeclaration {
        id: stable_id("native-apt-resolution"),
        version: config.apt_version.clone(),
        manager: PackageManager::Apt,
        source: config.apt_source_id.clone(),
        selector: Some(PackageSelector::AptBinary {
            name: config.apt_package.clone(),
            architecture: Some(config.target_arch.clone()),
        }),
    };
    assert_read_only_apt_plan(
        &apt_declaration,
        &apt_target,
        &apt_manager,
        &apt_registry,
        &apt_authority,
        &apt_repository,
        &config.apt_repository_url,
    );
    let mut apt_backend =
        AptResolutionBackend::new(apt_repository, ProcessAptResolutionCommandRunner);
    let apt_policy = package_policy(&config.apt_source_id);
    let apt_desired = redacted(
        PackageDesiredIntent::new(apt_declaration),
        "APT desired intent validation",
    );
    let apt_intent = redacted(
        PackageResolutionCoordinator::new(
            &apt_policy,
            &apt_registry,
            apt_manager.clone(),
            &mut apt_backend,
            &mut fetch,
        )
        .resolve(&apt_desired, &apt_target, &store),
        "APT controlled resolution",
    );
    let apt_reopened = redacted(
        apt_intent.load_and_validate(&apt_target, &apt_manager, &apt_registry, &store),
        "APT offline reopen",
    );
    println!(
        "native-resolution manager=apt os={} distro={} arch={} resolution={} closure={} artifacts={}",
        apt_target.os,
        apt_target.distro_id.as_deref().unwrap_or("unknown"),
        apt_target.arch,
        apt_intent.resolution.digest,
        apt_reopened.closure.len(),
        apt_reopened.artifacts.len()
    );

    let isolated_home = root.path().join("node-home");
    let isolated_nvm = isolated_home.join(".nvm");
    redacted(
        fs::create_dir_all(&isolated_nvm),
        "isolated nvm workspace creation",
    );
    let nvm_script = redacted(
        read_regular_no_follow(&config.node_nvm_script),
        "pinned nvm script read",
    );
    assert_generated_unsafe_nvm_control(
        root.path(),
        &nvm_script,
        &config.node_shell,
        &config.node_release_keyring,
        &config.apt_target(),
    );
    redacted(
        fs::write(isolated_nvm.join("nvm.sh"), nvm_script),
        "isolated nvm script copy",
    );
    let node_target = config.node_target(&isolated_nvm);
    let mut host_probe = ProcessNodeRuntimeHost::new(
        &isolated_nvm,
        &config.node_shell,
        &config.node_release_keyring,
    );
    let node_snapshot = redacted(host_probe.probe(&node_target), "nvm authority probe");
    let node_manager = node_snapshot.manager;
    let node_registry = redacted(
        PackageSourceRegistry::builtin().and_then(|registry| {
            registry.with_commonkit_node_release_authority(&config.node_source_id)
        }),
        "Node source registry construction",
    );
    let node_policy = package_policy(&config.node_source_id);
    let node_desired = redacted(
        PackageDesiredIntent::new(PackageDeclaration {
            id: stable_id("native-node-resolution"),
            version: config.node_version.clone(),
            manager: PackageManager::Nvm,
            source: config.node_source_id.clone(),
            selector: Some(PackageSelector::NodeRuntime {}),
        }),
        "Node desired intent validation",
    );
    let mut node_host = ProcessNodeRuntimeHost::new(
        &isolated_nvm,
        &config.node_shell,
        &config.node_release_keyring,
    );
    let mut verifier = ProcessNodeReleaseSignatureVerifier::new(&config.node_gpgv);
    let mut node_backend = NodeResolutionBackend::new(&mut node_host, &mut verifier);
    let node_intent = redacted(
        PackageResolutionCoordinator::new(
            &node_policy,
            &node_registry,
            node_manager.clone(),
            &mut node_backend,
            &mut fetch,
        )
        .resolve(&node_desired, &node_target, &store),
        "Node controlled resolution",
    );
    drop(node_backend);
    let node_reopened = redacted(
        node_intent.load_and_validate(&node_target, &node_manager, &node_registry, &store),
        "Node offline reopen",
    );
    println!(
        "native-resolution manager=nvm os={} arch={} resolution={} closure={} artifacts={}",
        node_target.os,
        node_target.arch,
        node_intent.resolution.digest,
        node_reopened.closure.len(),
        node_reopened.artifacts.len()
    );
}

#[cfg(unix)]
fn assert_generated_unsafe_nvm_control(
    root: &Path,
    nvm_script: &[u8],
    shell: &Path,
    release_keyring: &Path,
    target_template: &PackageTargetV1,
) {
    let unsafe_nvm = root.join("unsafe-nvm-home/.nvm");
    redacted(
        fs::create_dir_all(&unsafe_nvm),
        "unsafe nvm control workspace creation",
    );
    redacted(
        fs::write(unsafe_nvm.join("nvm.sh"), nvm_script),
        "unsafe nvm control script creation",
    );
    redacted(
        fs::write(unsafe_nvm.join("default-packages"), b"typescript\n"),
        "unsafe nvm control configuration creation",
    );
    let mut unsafe_target = target_template.clone();
    unsafe_target.manager_prefix = Some(path_text(&unsafe_nvm, "unsafe nvm control path"));
    let mut unsafe_host = ProcessNodeRuntimeHost::new(&unsafe_nvm, shell, release_keyring);

    assert_eq!(
        unsafe_host.probe(&unsafe_target),
        Err(NodeRuntimeHostError::UnsafeConfiguration(
            "nvm default-packages is not allowed".into()
        )),
        "generated unsafe nvm control did not reach the expected policy rejection"
    );
}

#[cfg(unix)]
fn assert_read_only_apt_plan(
    declaration: &PackageDeclaration,
    target: &PackageTargetV1,
    manager: &commonkit_adapters::ManagerBindingV1,
    registry: &PackageSourceRegistry,
    authority: &commonkit_adapters::AptSourceAuthorityV1,
    repository: &AptRepositoryConfigurationV1,
    canonical_repository: &str,
) {
    let request = AptResolutionSystemRequestV1 {
        declaration: declaration.clone(),
        target: target.clone(),
        manager: manager.clone(),
        source_id: repository.source_id.clone(),
        canonical_repository: canonical_repository.into(),
        registry_definition_digest: redacted(
            registry.source_definition_digest(&repository.source_id),
            "APT registry digest lookup",
        ),
        source_authority: authority.clone(),
        repository: repository.clone(),
    };
    let mut commands = redacted(
        ProcessAptResolutionCommandRunner::command_snapshot(&request),
        "APT read-only command plan",
    );
    let PackageSelector::AptBinary { name, architecture } = declaration
        .selector
        .as_ref()
        .expect("validated native APT selector")
    else {
        unreachable!("validated native APT selector")
    };
    let metadata_commands = redacted(
        ProcessAptResolutionCommandRunner::package_metadata_command_snapshot(&[
            AptResolvedPackageV1 {
                name: name.clone(),
                version: declaration.version.clone(),
                architecture: architecture.clone().unwrap_or_else(|| target.arch.clone()),
            },
        ]),
        "APT signed package metadata command plan",
    );
    commands.extend(metadata_commands);
    assert!(
        validate_read_only_apt_commands(&commands).is_ok(),
        "native evidence command plan could mutate target state"
    );
}

#[cfg(unix)]
fn validate_read_only_apt_commands(commands: &[AptCommandSpecV1]) -> Result<(), &'static str> {
    for command in commands {
        if command.environment
            != BTreeMap::from([("APT_CONFIG".into(), "<private>/apt.conf".into())])
        {
            return Err("command is not bound to the private APT configuration");
        }

        match command.executable.as_str() {
            "/usr/bin/apt-get" => validate_apt_get_command(command)?,
            "/usr/bin/apt-cache" => validate_apt_cache_command(command)?,
            "/usr/bin/dpkg"
                if !command.network
                    && matches!(command.args.as_slice(), [argument] if matches!(argument.as_str(), "--version" | "--print-architecture")) =>
                {}
            "/usr/bin/apt-config" if !command.network && command.args.as_slice() == ["dump"] => {}
            "/usr/bin/dpkg-query"
                if !command.network
                    && command.args.as_slice()
                        == [
                            "-W",
                            "-f=${binary:Package}\\t${db:Status-Abbrev}\\t${Version}\\t${Architecture}\\n",
                        ] => {}
            "/usr/bin/apt-mark" if !command.network && command.args.as_slice() == ["showhold"] => {}
            _ => return Err("command does not match a closed read-only APT evidence shape"),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_apt_get_command(command: &AptCommandSpecV1) -> Result<(), &'static str> {
    if !command.network && command.args.as_slice() == ["--version"] {
        return Ok(());
    }
    let (options, action) = parse_apt_options(&command.args)?;
    match action {
        [update] if command.network && update == "update" => {
            require_canonical_apt_options(&options, false)
        }
        [simulate, no_remove, no_recommends, install, package]
            if !command.network
                && simulate == "--simulate"
                && no_remove == "--no-remove"
                && no_recommends == "--no-install-recommends"
                && install == "install"
                && is_exact_package(package) =>
        {
            require_canonical_apt_options(&options, true)
        }
        [
            quiet,
            print_uris,
            download_only,
            no_remove,
            no_recommends,
            reinstall,
            install,
            package,
        ] if !command.network
            && quiet == "--quiet=2"
            && print_uris == "--print-uris"
            && download_only == "--download-only"
            && no_remove == "--no-remove"
            && no_recommends == "--no-install-recommends"
            && reinstall == "--reinstall"
            && install == "install"
            && is_exact_package(package) =>
        {
            require_canonical_apt_options(&options, true)
        }
        _ => Err("apt-get command does not match a closed read-only shape"),
    }
}

#[cfg(unix)]
fn validate_apt_cache_command(command: &AptCommandSpecV1) -> Result<(), &'static str> {
    let (options, action) = parse_apt_options(&command.args)?;
    match action {
        [depends, recurse, package]
            if !command.network
                && depends == "depends"
                && recurse == "--recurse"
                && is_exact_package(package) =>
        {
            require_canonical_apt_options(&options, true)
        }
        [show, package] if !command.network && show == "show" && is_exact_package(package) => {
            require_canonical_apt_options(&options, true)
        }
        _ => Err("apt-cache command does not match the closed dependency-inspection shape"),
    }
}

#[cfg(unix)]
fn parse_apt_options(args: &[String]) -> Result<(BTreeMap<&str, &str>, &[String]), &'static str> {
    let mut index = 0;
    let mut options = BTreeMap::new();
    while index < args.len() && args[index] == "-o" {
        let assignment = args
            .get(index + 1)
            .ok_or("APT -o is missing its key=value assignment")?;
        let (key, value) = assignment
            .split_once('=')
            .filter(|(key, value)| !key.is_empty() && !value.is_empty())
            .ok_or("APT -o assignment is malformed")?;
        if options.insert(key, value).is_some() {
            return Err("APT option is duplicated");
        }
        index += 2;
    }
    if args[index..].iter().any(|argument| argument == "-o") {
        return Err("APT option appears after the command action");
    }
    Ok((options, &args[index..]))
}

#[cfg(unix)]
fn require_canonical_apt_options(
    actual: &BTreeMap<&str, &str>,
    include_status: bool,
) -> Result<(), &'static str> {
    let mut expected = BTreeMap::from([
        ("Dir::Etc::sourcelist", "<private>/sources.list"),
        ("Dir::Etc::sourceparts", "-"),
        ("Dir::State::lists", "<private>/lists"),
        ("Dir::Cache::archives", "<private>/archives"),
        ("Dir::Cache::pkgcache", "<private>/pkgcache.bin"),
        ("Dir::Cache::srcpkgcache", "<private>/srcpkgcache.bin"),
        ("APT::Get::List-Cleanup", "0"),
        ("Acquire::AllowInsecureRepositories", "false"),
        ("Acquire::AllowWeakRepositories", "false"),
        ("Acquire::AllowDowngradeToInsecureRepositories", "false"),
        ("APT::Get::AllowUnauthenticated", "false"),
        ("Acquire::Check-Valid-Until", "true"),
        ("Acquire::Check-Date", "true"),
        ("Acquire::By-Hash", "force"),
        ("Acquire::http::AllowRedirect", "false"),
        ("Acquire::https::AllowRedirect", "false"),
        ("Acquire::http::Proxy", "DIRECT"),
        ("Acquire::https::Proxy", "DIRECT"),
        ("Acquire::Languages", "none"),
    ]);
    if include_status {
        expected.insert("Dir::State::status", "<private>/status");
    }
    if actual == &expected {
        Ok(())
    } else {
        Err("APT options differ from the closed private security profile")
    }
}

#[cfg(unix)]
fn is_exact_package(argument: &str) -> bool {
    argument.split_once('=').is_some_and(|(package, version)| {
        !package.is_empty()
            && !version.is_empty()
            && !package.starts_with('-')
            && !argument.chars().any(char::is_whitespace)
    })
}

#[cfg(unix)]
struct NativeHarnessConfig {
    target_os_version: String,
    target_distro_id: String,
    target_distro_version: String,
    target_codename: String,
    target_arch: String,
    apt_source_id: StableId,
    apt_repository_url: String,
    apt_suite: String,
    apt_components: BTreeSet<String>,
    apt_signing_key: PathBuf,
    apt_signing_authority: StableId,
    apt_package: String,
    apt_version: String,
    node_source_id: StableId,
    node_version: String,
    node_nvm_script: PathBuf,
    node_shell: PathBuf,
    node_release_keyring: PathBuf,
    node_gpgv: PathBuf,
}

#[cfg(unix)]
impl NativeHarnessConfig {
    fn from_environment() -> Self {
        Self {
            target_os_version: required_text("COMMONKIT_NATIVE_TARGET_OS_VERSION"),
            target_distro_id: required_text("COMMONKIT_NATIVE_TARGET_DISTRO_ID"),
            target_distro_version: required_text("COMMONKIT_NATIVE_TARGET_DISTRO_VERSION"),
            target_codename: required_text("COMMONKIT_NATIVE_TARGET_CODENAME"),
            target_arch: required_text("COMMONKIT_NATIVE_TARGET_ARCH"),
            apt_source_id: required_id("COMMONKIT_NATIVE_APT_SOURCE_ID"),
            apt_repository_url: required_text("COMMONKIT_NATIVE_APT_REPOSITORY"),
            apt_suite: required_text("COMMONKIT_NATIVE_APT_SUITE"),
            apt_components: required_text("COMMONKIT_NATIVE_APT_COMPONENTS")
                .split(',')
                .map(str::trim)
                .map(str::to_owned)
                .collect(),
            apt_signing_key: required_path("COMMONKIT_NATIVE_APT_SIGNING_KEY"),
            apt_signing_authority: required_id("COMMONKIT_NATIVE_APT_SIGNING_AUTHORITY"),
            apt_package: required_text("COMMONKIT_NATIVE_APT_PACKAGE"),
            apt_version: required_text("COMMONKIT_NATIVE_APT_VERSION"),
            node_source_id: required_id("COMMONKIT_NATIVE_NODE_SOURCE_ID"),
            node_version: required_text("COMMONKIT_NATIVE_NODE_VERSION"),
            node_nvm_script: required_path("COMMONKIT_NATIVE_NVM_SCRIPT"),
            node_shell: required_path("COMMONKIT_NATIVE_NODE_SHELL"),
            node_release_keyring: required_path("COMMONKIT_NATIVE_NODE_RELEASE_KEYRING"),
            node_gpgv: required_path("COMMONKIT_NATIVE_NODE_GPGV"),
        }
    }

    fn apt_target(&self) -> PackageTargetV1 {
        PackageTargetV1 {
            os: "linux".into(),
            os_version: self.target_os_version.clone(),
            distro_id: Some(self.target_distro_id.clone()),
            distro_version: Some(self.target_distro_version.clone()),
            codename: Some(self.target_codename.clone()),
            arch: self.target_arch.clone(),
            libc: Some("glibc".into()),
            manager_prefix: None,
        }
    }

    fn node_target(&self, nvm_dir: &Path) -> PackageTargetV1 {
        PackageTargetV1 {
            manager_prefix: Some(path_text(nvm_dir, "isolated nvm path")),
            ..self.apt_target()
        }
    }

    fn apt_repository(&self) -> AptRepositoryConfigurationV1 {
        AptRepositoryConfigurationV1 {
            source_id: self.apt_source_id.clone(),
            suite: self.apt_suite.clone(),
            components: self.apt_components.clone(),
            signed_by: self.apt_signing_key.clone(),
            signing_authority: self.apt_signing_authority.clone(),
        }
    }
}

#[cfg(unix)]
struct NativeHttpsFetch {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}

#[cfg(unix)]
impl NativeHttpsFetch {
    fn new() -> Result<Self, PackageResolutionError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PackageResolutionError::FetchUnavailable)?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .user_agent("commonkit-native-resolution-evidence/1")
            .build()
            .map_err(|_| PackageResolutionError::FetchUnavailable)?;
        Ok(Self { runtime, client })
    }

    fn fetch_one(
        &mut self,
        locator: &str,
        maximum_bytes: u64,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        let client = self.client.clone();
        let locator = locator.to_owned();
        self.runtime.block_on(async move {
            let url = validated_https_url(&locator)?;
            let response = client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| PackageResolutionError::FetchUnavailable)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(PackageResolutionError::UnvalidatedRedirect)?;
                let location = url
                    .join(location)
                    .map_err(|_| PackageResolutionError::UnvalidatedRedirect)?;
                return Ok(PackageFetchHopV1::Redirect {
                    location: location.into(),
                });
            }
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|length| length > maximum_bytes)
            {
                return Err(PackageResolutionError::FetchUnavailable);
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| PackageResolutionError::FetchUnavailable)?;
                let next = u64::try_from(bytes.len())
                    .ok()
                    .and_then(|length| length.checked_add(u64::try_from(chunk.len()).ok()?))
                    .ok_or(PackageResolutionError::CorruptArtifact)?;
                if next > maximum_bytes {
                    return Err(PackageResolutionError::CorruptArtifact);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(PackageFetchHopV1::Complete(PackageFetchResultV1 { bytes }))
        })
    }
}

#[cfg(unix)]
impl PackageFetch for NativeHttpsFetch {
    fn preflight_locators(&mut self, locators: &[String]) -> Result<(), PackageResolutionError> {
        for locator in locators {
            validated_https_url(locator)?;
        }
        Ok(())
    }

    fn fetch_hop(
        &mut self,
        request: &PackageFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.fetch_one(locator, request.size)
    }

    fn fetch_discovery_hop(
        &mut self,
        request: &PackageDiscoveryFetchRequestV1,
        locator: &str,
    ) -> Result<PackageFetchHopV1, PackageResolutionError> {
        self.fetch_one(locator, request.maximum_bytes)
    }
}

#[cfg(unix)]
fn validated_https_url(value: &str) -> Result<reqwest::Url, PackageResolutionError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| PackageResolutionError::UnapprovedArtifactLocation)?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PackageResolutionError::UnapprovedArtifactLocation);
    }
    Ok(url)
}

#[cfg(unix)]
fn required_text(name: &'static str) -> String {
    let value =
        std::env::var_os(name).unwrap_or_else(|| panic!("required harness input {name} is absent"));
    value
        .into_string()
        .unwrap_or_else(|_| panic!("required harness input {name} is not UTF-8"))
}

#[cfg(unix)]
fn required_path(name: &'static str) -> PathBuf {
    let path = PathBuf::from(required_text(name));
    assert!(
        path.is_absolute(),
        "required harness path {name} is not absolute"
    );
    path
}

#[cfg(unix)]
fn required_id(name: &'static str) -> StableId {
    redacted(
        StableId::parse(required_text(name)),
        "stable harness identifier",
    )
}

#[cfg(unix)]
fn stable_id(value: &str) -> StableId {
    StableId::parse(value).expect("static harness ID is valid")
}

#[cfg(unix)]
fn fixture_digest(byte: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", byte.to_string().repeat(64)))
        .expect("static fixture digest is valid")
}

#[cfg(unix)]
fn package_policy(source_id: &StableId) -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            stable_id("package_sources"),
            BTreeSet::from([source_id.as_str().to_owned()]),
        )]),
        ..SecurityPolicy::default()
    }
}

#[cfg(unix)]
fn path_text(path: &Path, stage: &'static str) -> String {
    path.to_str()
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("{stage} is not UTF-8"))
}

#[cfg(unix)]
fn read_regular_no_follow(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn redacted<T, E>(result: Result<T, E>, stage: &'static str) -> T {
    result.unwrap_or_else(|_| panic!("{stage} failed"))
}
