//! CI-only executable fixture proving restart recovery through an installed artifact.

use std::{env, fs, path::Path};

use commonkit_snapshots::{
    DatabaseId, DatabaseLifecycle, DurableObjectStore, DurableRestore, RestoreFailpoint,
    RestorePlan, RestoreState, SnapshotError, SnapshotService, StaticBackup, XChaCha20Cipher,
    manifest_digest,
};
use sha2::{Digest, Sha256};

struct NoopLifecycle;

impl DatabaseLifecycle for NoopLifecycle {
    fn stop(&mut self) -> Result<(), SnapshotError> {
        Ok(())
    }

    fn start(&mut self) -> Result<(), SnapshotError> {
        Ok(())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("commonkit-snapshot-recovery-fixture: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let action = arguments
        .next()
        .ok_or("expected interrupt, sigkill, or recover")?;
    let root = arguments.next().ok_or("expected fixture root")?;
    if arguments.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    let root = Path::new(&root);
    fs::create_dir_all(root)?;
    let database_path = root.join("database.bin");
    // Match the production file-reference derivation used by the snapshot domain.
    let mut key = [0_u8; 32];
    key.copy_from_slice(&Sha256::digest([47_u8; 32]));
    let cipher = XChaCha20Cipher::new(key);
    let transaction_root = root.join("transactions");

    match action.to_str() {
        Some(action @ ("interrupt" | "sigkill")) => {
            fs::write(&database_path, b"pre-interruption database")?;
            let mut objects = DurableObjectStore::open(root.join("objects"))?;
            let database = DatabaseId::new("context-mode")?;
            let manifest = SnapshotService::new(&cipher).snapshot(
                &database,
                "local",
                None,
                &StaticBackup::new("file", b"staged restored database".to_vec()),
                &mut objects,
            )?;
            let plan = RestorePlan {
                schema: "commonkit.restore-plan.v1".into(),
                run_id: "installed-interrupted-restore".into(),
                snapshot_id: "installed-fixture-snapshot".into(),
                manifest_digest: manifest_digest(&manifest)?,
                database,
                expected_content_digest: manifest.content_digest.clone(),
            };
            let failpoint = if action == "sigkill" {
                RestoreFailpoint::KillAfterSwap
            } else {
                RestoreFailpoint::AfterSwap
            };
            let result = DurableRestore::open(transaction_root, &cipher)?.execute(
                plan,
                &manifest,
                &objects,
                &database_path,
                &mut NoopLifecycle,
                failpoint,
            );
            if action == "sigkill" {
                return Err("SIGKILL failpoint returned unexpectedly".into());
            }
            if !matches!(result, Err(SnapshotError::Interrupted)) {
                return Err("restore did not stop at the requested interruption".into());
            }
            if fs::read(&database_path)? != b"staged restored database" {
                return Err("interrupted restore did not reach the swapped state".into());
            }
        }
        Some("recover") => {
            let receipt = DurableRestore::open(transaction_root, &cipher)?.recover(
                "installed-interrupted-restore",
                &database_path,
                &mut NoopLifecycle,
            )?;
            if receipt.state != RestoreState::RolledBack {
                return Err("fresh-process recovery did not roll back".into());
            }
            if fs::read(&database_path)? != b"pre-interruption database" {
                return Err("fresh-process recovery did not restore the preimage".into());
            }
        }
        _ => return Err("expected interrupt, sigkill, or recover".into()),
    }
    Ok(())
}
