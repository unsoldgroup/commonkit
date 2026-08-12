use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
enum ProofError {
    Unavailable,
    Failed(String),
}

fn native_executable() -> Result<PathBuf, ProofError> {
    if std::env::var("COMMONKIT_ENGRAM_NATIVE_PROOF").as_deref() != Ok("1") {
        return Err(ProofError::Unavailable);
    }
    let path = std::env::var_os("COMMONKIT_ENGRAM_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("engram"));
    if path.is_absolute() && !path.is_file() {
        return Err(ProofError::Unavailable);
    }
    Ok(path)
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
        .map_err(|error| ProofError::Failed(error.to_string()))?;
    if !output.status.success() {
        return Err(ProofError::Failed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), ProofError> {
    fs::create_dir_all(destination).map_err(|error| ProofError::Failed(error.to_string()))?;
    for entry in fs::read_dir(source).map_err(|error| ProofError::Failed(error.to_string()))? {
        let entry = entry.map_err(|error| ProofError::Failed(error.to_string()))?;
        let target = destination.join(entry.file_name());
        let metadata = entry
            .metadata()
            .map_err(|error| ProofError::Failed(error.to_string()))?;
        if metadata.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), target)
                .map_err(|error| ProofError::Failed(error.to_string()))?;
        }
    }
    Ok(())
}

fn native_import_idempotency_proof() -> Result<(), ProofError> {
    let executable = native_executable()?;
    let directory = tempfile::tempdir().map_err(|error| ProofError::Failed(error.to_string()))?;
    let source_data = directory.path().join("source-data");
    let destination_data = directory.path().join("destination-data");
    let source_project = directory.path().join("source-project");
    let destination_project = directory.path().join("destination-project");
    fs::create_dir_all(&source_project).map_err(|error| ProofError::Failed(error.to_string()))?;
    fs::create_dir_all(&destination_project)
        .map_err(|error| ProofError::Failed(error.to_string()))?;

    run(
        &executable,
        &source_data,
        &source_project,
        &[
            "save",
            "commonkit-native-proof",
            "synthetic-only-proof-payload",
            "--project",
            "commonkit-native-proof",
        ],
    )?;
    run(
        &executable,
        &source_data,
        &source_project,
        &["sync", "--project", "commonkit-native-proof"],
    )?;
    copy_tree(
        &source_project.join(".engram"),
        &destination_project.join(".engram"),
    )?;
    run(
        &executable,
        &destination_data,
        &destination_project,
        &["sync", "--import", "--project", "commonkit-native-proof"],
    )?;
    run(
        &executable,
        &destination_data,
        &destination_project,
        &["sync", "--import", "--project", "commonkit-native-proof"],
    )?;
    let search = run(
        &executable,
        &destination_data,
        &destination_project,
        &[
            "search",
            "commonkit-native-proof",
            "--project",
            "commonkit-native-proof",
            "--limit",
            "100",
        ],
    )?;
    if !search.contains("Found 1 memories") {
        return Err(ProofError::Failed(search));
    }
    Ok(())
}

#[test]
fn native_import_idempotency_proof_has_a_distinct_unavailable_state() {
    if std::env::var("COMMONKIT_ENGRAM_NATIVE_PROOF").is_ok() {
        return;
    }
    assert_eq!(
        native_import_idempotency_proof(),
        Err(ProofError::Unavailable)
    );
}

#[test]
fn native_import_idempotency_proof_is_opt_in_and_synthetic_only() {
    if std::env::var("COMMONKIT_ENGRAM_NATIVE_PROOF").as_deref() != Ok("1") {
        return;
    }
    native_import_idempotency_proof().unwrap();
}
