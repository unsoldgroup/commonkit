//! Layer loading and composition.

use std::collections::BTreeSet;

use commonkit_contracts::{LayerDocument, LayerKind};
use thiserror::Error;

pub struct LayerSet {
    layers: Vec<LayerDocument>,
}

impl LayerSet {
    pub fn new(mut layers: Vec<LayerDocument>) -> Result<Self, LayerSetError> {
        let mut seen = BTreeSet::new();
        for layer in &layers {
            if !seen.insert(layer.kind) {
                return Err(LayerSetError::DuplicateKind(layer.kind));
            }
        }
        for required in [LayerKind::PublicBase, LayerKind::OrganizationPolicy] {
            if !seen.contains(&required) {
                return Err(LayerSetError::MissingRequired(required));
            }
        }
        layers.sort_by_key(|layer| precedence(layer.kind));
        Ok(Self { layers })
    }

    pub fn iter(&self) -> impl Iterator<Item = &LayerDocument> {
        self.layers.iter()
    }
}

fn precedence(kind: LayerKind) -> u8 {
    match kind {
        LayerKind::PublicBase => 0,
        LayerKind::OrganizationPolicy => 1,
        LayerKind::PersonalKit => 2,
        LayerKind::ProjectLoadout => 3,
        LayerKind::TargetOverrides => 4,
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LayerSetError {
    #[error("required layer kind is missing: {0:?}")]
    MissingRequired(LayerKind),
    #[error("layer kind appears more than once: {0:?}")]
    DuplicateKind(LayerKind),
}
