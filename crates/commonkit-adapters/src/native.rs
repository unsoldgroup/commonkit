use commonkit_contracts::StableId;

use super::{
    ArtifactStore, DesiredStateProvider, MaterializedState, NormalizedResource, ProviderContext,
    ProviderFailure, ProviderInputs, ProviderWorkspace,
};

/// Migration/fallback provider for resources already composed by CommonKit.
#[derive(Debug, Clone)]
pub struct NativeProvider {
    id: StableId,
    inputs: ProviderInputs,
    resources: Vec<NormalizedResource>,
}

impl NativeProvider {
    pub fn new(
        inputs: ProviderInputs,
        resources: Vec<NormalizedResource>,
    ) -> Result<Self, ProviderFailure> {
        if inputs.provider_id.as_str() != "native" {
            return Err(ProviderFailure::Inspect(
                "native provider inputs must use provider ID native".into(),
            ));
        }
        let state =
            MaterializedState::finalize(inputs.clone(), resources.clone(), Vec::new(), Vec::new())?;
        Ok(Self {
            id: inputs.provider_id.clone(),
            inputs,
            resources: state.resources,
        })
    }
}

impl DesiredStateProvider for NativeProvider {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn inspect_inputs(
        &self,
        _context: &ProviderContext,
    ) -> Result<ProviderInputs, ProviderFailure> {
        self.inputs.verify()?;
        Ok(self.inputs.clone())
    }

    fn materialize(
        &self,
        _context: &ProviderContext,
        _workspace: &ProviderWorkspace,
        _artifacts: &ArtifactStore,
    ) -> Result<MaterializedState, ProviderFailure> {
        MaterializedState::finalize(
            self.inputs.clone(),
            self.resources.clone(),
            Vec::new(),
            Vec::new(),
        )
        .map_err(Into::into)
    }
}
