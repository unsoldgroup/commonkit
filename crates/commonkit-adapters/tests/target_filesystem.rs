use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{
    FileMode, LocalTargetFilesystem, NormalizedManagedPath, SafeSymlinkTarget,
    SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport, SymlinkTargetKind,
    TargetFilesystem, TargetFilesystemError, TargetResource,
};
use commonkit_core::RootAccess;

fn temp(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "commonkit-target-{name}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn local_filesystem_is_confined_to_one_declared_capability_root() {
    let root = temp("local");
    fs::create_dir_all(&root).unwrap();
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadWrite).unwrap();
    let path = NormalizedManagedPath::parse("config/tool/settings.json").unwrap();

    target.write_file(&path, b"{}\n").unwrap();
    assert_eq!(target.read_file(&path).unwrap(), Some(b"{}\n".to_vec()));
    assert_eq!(
        fs::read(root.join("config/tool/settings.json")).unwrap(),
        b"{}\n"
    );

    drop(target);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn local_filesystem_replaces_existing_file_via_a_durable_staged_write() {
    use std::os::unix::fs::MetadataExt;

    let root = temp("atomic-write");
    fs::create_dir_all(&root).unwrap();
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadWrite).unwrap();
    let path = NormalizedManagedPath::parse("manifest.json").unwrap();

    target.write_file(&path, b"old").unwrap();
    let first_inode = fs::metadata(root.join("manifest.json")).unwrap().ino();
    target.write_file(&path, b"new").unwrap();
    let second_inode = fs::metadata(root.join("manifest.json")).unwrap().ino();

    assert_ne!(first_inode, second_inode);
    assert_eq!(target.read_file(&path).unwrap(), Some(b"new".to_vec()));
    assert!(!root.join(".manifest.json.commonkit-tmp").exists());

    drop(target);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn local_filesystem_rejects_symlink_ancestors_and_leafs() {
    use std::os::unix::fs::symlink;

    let root = temp("symlink-root");
    let outside = temp("symlink-outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret"), b"outside").unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    symlink(outside.join("secret"), root.join("leaf")).unwrap();
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadWrite).unwrap();

    let ancestor = NormalizedManagedPath::parse("escape/new").unwrap();
    let leaf = NormalizedManagedPath::parse("leaf").unwrap();
    assert!(matches!(
        target.write_file(&ancestor, b"bad"),
        Err(TargetFilesystemError::SymlinkEncountered(_))
    ));
    assert!(matches!(
        target.read_file(&leaf),
        Err(TargetFilesystemError::SymlinkEncountered(_))
    ));
    assert!(!outside.join("new").exists());

    drop(target);
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn read_only_root_refuses_mutation() {
    let root = temp("readonly");
    fs::create_dir_all(&root).unwrap();
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadOnly).unwrap();
    let path = NormalizedManagedPath::parse("settings.json").unwrap();
    assert!(matches!(
        target.write_file(&path, b"{}"),
        Err(TargetFilesystemError::ReadOnly)
    ));
    drop(target);
    fs::remove_dir_all(root).unwrap();
}

#[derive(Default)]
struct FakeSsh {
    requests: Vec<SshFilesystemRequest>,
}

impl SshFilesystemTransport for FakeSsh {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        self.requests.push(request);
        Ok(SshFilesystemResponse::Absent)
    }
}

#[test]
fn ssh_boundary_exposes_typed_filesystem_requests() {
    let mut ssh = FakeSsh::default();
    let request = SshFilesystemRequest::ReadFile {
        root_id: commonkit_core::StableId::parse("home").unwrap(),
        path: NormalizedManagedPath::parse("config/tool").unwrap(),
    };
    assert_eq!(
        ssh.perform(request.clone()).unwrap(),
        SshFilesystemResponse::Absent
    );
    assert_eq!(ssh.requests, vec![request]);
}
#[cfg(unix)]
#[test]
fn local_filesystem_observes_and_applies_relative_directories_and_symlinks_without_following() {
    let root = temp("typed-resources");
    fs::create_dir_all(&root).unwrap();
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadWrite).unwrap();
    let source = NormalizedManagedPath::parse(".agents/skills/tool").unwrap();
    let link = NormalizedManagedPath::parse(".codex/skills/tool").unwrap();
    let relative = SafeSymlinkTarget::parse(&link, "../../.agents/skills/tool").unwrap();

    target
        .write_directory(&source, Some(&FileMode::parse(0o700).unwrap()))
        .unwrap();
    target
        .write_symlink(&link, &relative, SymlinkTargetKind::Directory)
        .unwrap();

    assert!(matches!(
        target.inspect_resource(&source).unwrap(),
        TargetResource::Directory { mode: Some(0o700) }
    ));
    assert_eq!(
        target.inspect_resource(&link).unwrap(),
        TargetResource::Symlink {
            target: "../../.agents/skills/tool".into(),
            target_kind: SymlinkTargetKind::File,
        }
    );
    assert!(matches!(
        target.read_file(&link),
        Err(TargetFilesystemError::SymlinkEncountered(_))
    ));

    let dangling = NormalizedManagedPath::parse(".codex/skills/dangling").unwrap();
    std::os::unix::fs::symlink("../../.agents/skills/missing", root.join(dangling.as_str()))
        .unwrap();
    assert_eq!(
        target.inspect_resource(&dangling).unwrap(),
        TargetResource::Symlink {
            target: "../../.agents/skills/missing".into(),
            target_kind: SymlinkTargetKind::File,
        }
    );

    drop(target);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ssh_boundary_exposes_typed_relative_directory_and_symlink_requests() {
    let mut ssh = FakeSsh::default();
    let link = NormalizedManagedPath::parse(".codex/skills/tool").unwrap();
    let requests = vec![
        SshFilesystemRequest::InspectResource {
            root_id: commonkit_core::StableId::parse("home").unwrap(),
            path: link.clone(),
        },
        SshFilesystemRequest::WriteDirectory {
            root_id: commonkit_core::StableId::parse("home").unwrap(),
            path: NormalizedManagedPath::parse(".agents/skills/tool").unwrap(),
            mode: Some(FileMode::parse(0o700).unwrap()),
        },
        SshFilesystemRequest::WriteSymlink {
            root_id: commonkit_core::StableId::parse("home").unwrap(),
            path: link.clone(),
            target: SafeSymlinkTarget::parse(&link, "../../.agents/skills/tool").unwrap(),
            target_kind: SymlinkTargetKind::Directory,
        },
    ];
    for request in &requests {
        assert_eq!(
            ssh.perform(request.clone()).unwrap(),
            SshFilesystemResponse::Absent
        );
    }
    assert_eq!(ssh.requests, requests);
}

#[cfg(not(unix))]
#[test]
fn local_filesystem_rejects_modes_the_target_cannot_apply() {
    let root = temp("unsupported-mode");
    fs::create_dir_all(&root).unwrap();
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadWrite).unwrap();
    let path = NormalizedManagedPath::parse("config").unwrap();

    assert!(matches!(
        target.write_directory(&path, Some(&FileMode::parse(0o700).unwrap())),
        Err(TargetFilesystemError::UnsupportedMode)
    ));
    assert!(!root.join("config").exists());

    drop(target);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn local_filesystem_rejects_junction_ancestors() {
    let root = temp("junction-root");
    let root_junction = temp("junction-root-capability");
    let outside = temp("junction-outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let junction = |link: &std::path::Path| {
        let status = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                link.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
    };
    junction(&root_junction);
    junction(&root.join("escape"));
    junction(&root.join("leaf"));
    assert!(matches!(
        LocalTargetFilesystem::open(&root_junction, RootAccess::ReadWrite),
        Err(TargetFilesystemError::InvalidRoot)
    ));
    assert!(
        !fs::symlink_metadata(root.join("escape"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let target = LocalTargetFilesystem::open(&root, RootAccess::ReadWrite).unwrap();

    assert!(matches!(
        target.write_file(&NormalizedManagedPath::parse("escape/new").unwrap(), b"bad"),
        Err(TargetFilesystemError::SymlinkEncountered(_))
    ));
    assert!(!outside.join("new").exists());
    assert!(matches!(
        target.remove_resource(&NormalizedManagedPath::parse("leaf").unwrap()),
        Err(TargetFilesystemError::Io(_))
    ));
    assert!(outside.exists());

    drop(target);
    fs::remove_dir(root_junction).unwrap();
    fs::remove_dir(root.join("escape")).unwrap();
    fs::remove_dir(root.join("leaf")).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}
