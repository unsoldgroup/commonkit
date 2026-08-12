use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
enum ProofError {
    Unavailable(&'static str),
    Failed(&'static str),
}

fn native_executable() -> Result<PathBuf, ProofError> {
    if std::env::var("COMMONKIT_ENGRAM_NATIVE_PROOF").as_deref() != Ok("1") {
        return Err(ProofError::Unavailable("native proof is opt-in"));
    }
    let path = std::env::var_os("COMMONKIT_ENGRAM_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("engram"));
    if path.is_absolute() {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ProofError::Unavailable("configured Engram executable is absent"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ProofError::Unavailable(
                "configured Engram executable is not a regular file",
            ));
        }
    }
    Ok(path)
}

fn configured_source() -> Result<PathBuf, ProofError> {
    let Some(source) = std::env::var_os("COMMONKIT_ENGRAM_NATIVE_SOURCE") else {
        return Err(ProofError::Unavailable(
            "existing Engram data store source is not configured",
        ));
    };
    let source = PathBuf::from(source);
    let metadata = fs::symlink_metadata(&source)
        .map_err(|_| ProofError::Unavailable("configured Engram data store source is absent"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ProofError::Unavailable(
            "configured Engram data store source is absent",
        ));
    }
    Ok(source)
}

fn run(
    executable: &Path,
    data_dir: &Path,
    project_dir: &Path,
    arguments: &[&str],
) -> Result<String, ProofError> {
    let output = Command::new(executable)
        .args(arguments)
        .current_dir(project_dir)
        .env("ENGRAM_DATA_DIR", data_dir)
        .output()
        .map_err(|_| ProofError::Failed("failed to launch native Engram"))?;
    if !output.status.success() {
        return Err(ProofError::Failed("native Engram command failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Copies only regular files and directories. The configured source is never
/// passed to Engram and is never opened for writing.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), ProofError> {
    fs::create_dir_all(destination)
        .map_err(|_| ProofError::Failed("failed to copy source store"))?;
    for entry in
        fs::read_dir(source).map_err(|_| ProofError::Failed("failed to read source store"))?
    {
        let entry = entry.map_err(|_| ProofError::Failed("failed to read source store"))?;
        let target = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| ProofError::Failed("failed to inspect source store"))?;
        if metadata.file_type().is_symlink() {
            return Err(ProofError::Failed("source store contains a symlink"));
        }
        if metadata.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), target)
                .map_err(|_| ProofError::Failed("failed to copy source store"))?;
        } else {
            return Err(ProofError::Failed("source store contains an unsafe entry"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_read_only_tree(root: &Path) -> Result<(), ProofError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| ProofError::Failed("failed to inspect copied store"))?;
    if metadata.is_dir() {
        for entry in
            fs::read_dir(root).map_err(|_| ProofError::Failed("failed to inspect copied store"))?
        {
            make_read_only_tree(
                &entry
                    .map_err(|_| ProofError::Failed("failed to inspect copied store"))?
                    .path(),
            )?;
        }
        fs::set_permissions(root, fs::Permissions::from_mode(0o500))
            .map_err(|_| ProofError::Failed("failed to protect copied store"))?;
    } else if metadata.is_file() {
        fs::set_permissions(root, fs::Permissions::from_mode(0o400))
            .map_err(|_| ProofError::Failed("failed to protect copied store"))?;
    }
    Ok(())
}

fn native_import_idempotency_proof() -> Result<(), ProofError> {
    let executable = native_executable()?;
    let source = configured_source()?;
    let directory = tempfile::tempdir()
        .map_err(|_| ProofError::Failed("failed to create proof temp directory"))?;
    let copied_data = directory.path().join("copied-data");
    let destination_data = directory.path().join("destination-data");
    let source_project = directory.path().join("source-project");
    let destination_project = directory.path().join("destination-project");
    fs::create_dir_all(&source_project)
        .map_err(|_| ProofError::Failed("failed to create proof project"))?;
    fs::create_dir_all(&destination_project)
        .map_err(|_| ProofError::Failed("failed to create proof project"))?;
    copy_tree(&source, &copied_data)?;
    #[cfg(unix)]
    make_read_only_tree(&copied_data)?;

    let project_id = std::env::var("COMMONKIT_ENGRAM_NATIVE_PROJECT")
        .map_err(|_| ProofError::Unavailable("native proof project identity is not configured"))?;
    let query = std::env::var("COMMONKIT_ENGRAM_NATIVE_QUERY")
        .map_err(|_| ProofError::Unavailable("native proof query is not configured"))?;
    let project_id_arg = project_id.as_str();
    run(
        &executable,
        &copied_data,
        &source_project,
        &["sync", "--project", project_id_arg],
    )?;
    copy_tree(
        &source_project.join(".engram"),
        &destination_project.join(".engram"),
    )?;
    run(
        &executable,
        &destination_data,
        &destination_project,
        &["sync", "--import", "--project", project_id_arg],
    )?;
    run(
        &executable,
        &destination_data,
        &destination_project,
        &["sync", "--import", "--project", project_id_arg],
    )?;
    let before = run(
        &executable,
        &destination_data,
        &destination_project,
        &[
            "search",
            query.as_str(),
            "--project",
            project_id_arg,
            "--limit",
            "100",
        ],
    )?;
    let after = run(
        &executable,
        &destination_data,
        &destination_project,
        &[
            "search",
            query.as_str(),
            "--project",
            project_id_arg,
            "--limit",
            "100",
        ],
    )?;
    if before != after {
        return Err(ProofError::Failed(
            "native repeated import changed the search result",
        ));
    }
    Ok(())
}

#[test]
fn native_import_idempotency_proof_has_a_distinct_unavailable_state() {
    if std::env::var("COMMONKIT_ENGRAM_NATIVE_PROOF").as_deref() != Ok("1") {
        return;
    }
    if std::env::var_os("COMMONKIT_ENGRAM_NATIVE_SOURCE").is_some() {
        return;
    }
    assert_eq!(
        native_import_idempotency_proof(),
        Err(ProofError::Unavailable(
            "existing Engram data store source is not configured",
        ))
    );
}

#[test]
fn native_import_idempotency_proof_is_opt_in_and_copy_only() {
    if std::env::var("COMMONKIT_ENGRAM_NATIVE_PROOF").as_deref() != Ok("1") {
        return;
    }
    match native_import_idempotency_proof() {
        Ok(()) | Err(ProofError::Unavailable(_)) => {}
        Err(ProofError::Failed(reason)) => panic!("{reason}"),
    }
}
