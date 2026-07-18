use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{
    LocalTargetFilesystem, NormalizedManagedPath, SshFilesystemRequest, SshFilesystemResponse,
    SshFilesystemTransport, TargetFilesystem, TargetFilesystemError,
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
