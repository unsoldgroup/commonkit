use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{
    ArtifactStore, DesiredStateProvider, MaterializedState, OwnershipRules, ProviderContext,
    ProviderContractError, ProviderFailure, ProviderWorkspace, validate_ownership,
};

/// Provider-neutral controller pipeline. Providers only see an isolated workspace; their
/// normalized output is ownership-validated and persisted by digest before any adapter plans a
/// live mutation.
pub struct ProviderPipeline {
    root: PathBuf,
    artifacts: ArtifactStore,
    ownership_rules: OwnershipRules,
    protected_roots: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct ProviderPipelineOutput {
    pub states: Vec<MaterializedState>,
    pub state_paths: Vec<PathBuf>,
}

impl ProviderPipeline {
    pub fn open(
        root: impl AsRef<Path>,
        artifacts: ArtifactStore,
        ownership_rules: OwnershipRules,
        protected_roots: Vec<PathBuf>,
    ) -> Result<Self, ProviderPipelineError> {
        let root = root.as_ref();
        if !root.is_absolute() || root.parent().is_none() {
            return Err(ProviderPipelineError::UnsafeRoot);
        }
        fs::create_dir_all(root.join("workspaces"))?;
        fs::create_dir_all(root.join("states"))?;
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ProviderPipelineError::UnsafeRoot);
        }
        Ok(Self {
            root: root.to_path_buf(),
            artifacts,
            ownership_rules,
            protected_roots,
        })
    }

    pub fn materialize_all(
        &self,
        providers: &[&dyn DesiredStateProvider],
        context: &ProviderContext,
    ) -> Result<ProviderPipelineOutput, ProviderPipelineError> {
        let mut states = Vec::with_capacity(providers.len());
        for provider in providers {
            if states.iter().any(|state: &MaterializedState| {
                state.inputs.provider_id == *provider.id()
            }) {
                return Err(ProviderPipelineError::DuplicateProvider(
                    provider.id().to_string(),
                ));
            }
            let workspace_root = self.root.join("workspaces").join(provider.id().as_str());
            reset_private_workspace(&workspace_root)?;
            let workspace = ProviderWorkspace::open(&workspace_root, &self.protected_roots)?;
            let inspected = provider.inspect_inputs(context)?;
            let state = provider.materialize(context, &workspace, &self.artifacts)?;
            state.verify()?;
            if state.inputs != inspected {
                return Err(ProviderPipelineError::InputsChanged(provider.id().to_string()));
            }
            states.push(state);
        }
        let resources = states
            .iter()
            .flat_map(|state| state.resources.iter().cloned())
            .collect::<Vec<_>>();
        validate_ownership(&resources, &self.ownership_rules)?;
        let state_paths = states
            .iter()
            .map(|state| self.persist_state(state))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProviderPipelineOutput {
            states,
            state_paths,
        })
    }

    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    fn persist_state(&self, state: &MaterializedState) -> Result<PathBuf, ProviderPipelineError> {
        let name = state.digest.as_str().trim_start_matches("sha256:");
        let path = self.root.join("states").join(format!("{name}.json"));
        let bytes = serde_json::to_vec(state)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(&path)? != bytes {
                    return Err(ProviderPipelineError::StateCollision);
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(path)
    }
}

fn reset_private_workspace(path: &Path) -> Result<(), ProviderPipelineError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ProviderPipelineError::UnsafeRoot);
        }
        Ok(_) => fs::remove_dir_all(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir(path)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProviderPipelineError {
    #[error("provider pipeline root must be an absolute non-symlink directory")]
    UnsafeRoot,
    #[error("provider appears more than once in the pipeline: {0}")]
    DuplicateProvider(String),
    #[error("provider inputs changed between inspection and isolated materialization: {0}")]
    InputsChanged(String),
    #[error("digest-addressed provider state already exists with different bytes")]
    StateCollision,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Provider(#[from] ProviderFailure),
    #[error(transparent)]
    Contract(#[from] ProviderContractError),
    #[error(transparent)]
    Ownership(#[from] crate::OwnershipError),
}
