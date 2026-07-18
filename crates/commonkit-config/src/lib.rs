//! Layer loading and composition.

use std::collections::{BTreeMap, BTreeSet};

use commonkit_contracts::{
    CommonKitLock, ContractError, Contribution, LayerDocument, LayerKind, LockedLayer,
    MergeOperation, ProvenanceTrace, SCHEMA_VERSION, SchemaVersion, Sha256Digest, TraceEntry,
    canonical_json, digest_json,
};
use serde_json::{Map, Value};
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

pub struct CompositionResult {
    pub spec: Value,
    pub spec_digest: Sha256Digest,
    pub trace: ProvenanceTrace,
    pub lock: CommonKitLock,
}

pub fn compose_layers(
    layers: &LayerSet,
    rules: &MergeRules,
) -> Result<CompositionResult, ComposeError> {
    let mut spec = Value::Object(Map::new());
    let mut contributions: BTreeMap<String, Vec<Contribution>> = BTreeMap::new();

    for layer in layers.iter() {
        collect_contributions(&layer.spec, "", &spec, layer, &mut contributions)?;
        spec = merge_specs(&spec, &layer.spec, rules)?;
    }
    let spec_digest = digest_json(&spec)?;
    let entries = contributions
        .into_iter()
        .filter_map(|(pointer, contributions)| {
            contributions.last().cloned().map(|winner| {
                (
                    pointer,
                    TraceEntry {
                        winner,
                        contributions,
                        governing_rules: Vec::new(),
                    },
                )
            })
        })
        .collect();
    let locked_layers = layers
        .iter()
        .map(|layer| LockedLayer {
            id: layer.id.clone(),
            kind: layer.kind,
            path: layer.source.path.clone(),
            schema_version: layer.schema_version,
            revision: layer.source.revision.clone(),
            content_digest: layer.source.content_digest.clone(),
        })
        .collect();
    Ok(CompositionResult {
        spec,
        spec_digest: spec_digest.clone(),
        trace: ProvenanceTrace {
            schema_version: SchemaVersion(SCHEMA_VERSION),
            state_digest: spec_digest.clone(),
            entries,
        },
        lock: CommonKitLock {
            schema_version: SchemaVersion(SCHEMA_VERSION),
            contract_version: commonkit_contracts::CONTRACT_VERSION.into(),
            repository_revision: None,
            layers: locked_layers,
            normalized_digest: spec_digest,
        },
    })
}

fn collect_contributions(
    overlay: &Value,
    pointer: &str,
    current: &Value,
    layer: &LayerDocument,
    contributions: &mut BTreeMap<String, Vec<Contribution>>,
) -> Result<(), ContractError> {
    if let Some(object) = overlay.as_object()
        && !object.is_empty()
        && !is_delete(overlay)
    {
        for (key, value) in object {
            let child_pointer = format!("{pointer}/{}", escape_pointer(key));
            collect_contributions(value, &child_pointer, current, layer, contributions)?;
        }
        return Ok(());
    }
    if pointer.is_empty() {
        return Ok(());
    }
    let operation = if is_delete(overlay) {
        MergeOperation::Delete
    } else if current.pointer(pointer).is_some() {
        MergeOperation::Replace
    } else {
        MergeOperation::Set
    };
    let value_digest = if operation == MergeOperation::Delete {
        None
    } else {
        Some(digest_json(overlay)?)
    };
    contributions
        .entry(pointer.into())
        .or_default()
        .push(Contribution {
            layer_id: layer.id.clone(),
            layer_kind: layer.kind,
            source: layer.source.clone(),
            operation,
            value_digest,
        });
    Ok(())
}

#[derive(Debug, Error)]
pub enum ComposeError {
    #[error(transparent)]
    Merge(#[from] MergeError),
    #[error(transparent)]
    Contract(#[from] ContractError),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeStrategy {
    Replace,
    RecursiveMap,
    SetUnion,
    MergeById { id_key: String },
}

#[derive(Debug, Clone, Default)]
pub struct MergeRules {
    strategies: BTreeMap<String, MergeStrategy>,
    deletable: BTreeSet<String>,
}

impl MergeRules {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_strategy(mut self, pointer: impl Into<String>, strategy: MergeStrategy) -> Self {
        self.strategies.insert(pointer.into(), strategy);
        self
    }

    pub fn allow_delete(mut self, pointer: impl Into<String>) -> Self {
        self.deletable.insert(pointer.into());
        self
    }

    fn strategy(&self, pointer: &str, base: &Value, overlay: &Value) -> MergeStrategy {
        self.strategies.get(pointer).cloned().unwrap_or_else(|| {
            if base.is_object() && overlay.is_object() {
                MergeStrategy::RecursiveMap
            } else {
                MergeStrategy::Replace
            }
        })
    }
}

pub fn merge_specs(base: &Value, overlay: &Value, rules: &MergeRules) -> Result<Value, MergeError> {
    merge_value(base, overlay, "", rules)?
        .ok_or_else(|| MergeError::DeleteNotAllowed { pointer: "".into() })
}

fn merge_value(
    base: &Value,
    overlay: &Value,
    pointer: &str,
    rules: &MergeRules,
) -> Result<Option<Value>, MergeError> {
    if is_delete(overlay) {
        return if rules.deletable.contains(pointer) {
            Ok(None)
        } else {
            Err(MergeError::DeleteNotAllowed {
                pointer: pointer.into(),
            })
        };
    }

    match rules.strategy(pointer, base, overlay) {
        MergeStrategy::Replace => Ok(Some(overlay.clone())),
        MergeStrategy::RecursiveMap => {
            let Some(base) = base.as_object() else {
                return Ok(Some(overlay.clone()));
            };
            let Some(overlay) = overlay.as_object() else {
                return Ok(Some(Value::Object(base.clone())));
            };
            let mut merged = base.clone();
            for (key, value) in overlay {
                let child_pointer = format!("{pointer}/{}", escape_pointer(key));
                let previous = merged.get(key).unwrap_or(&Value::Null);
                match merge_value(previous, value, &child_pointer, rules)? {
                    Some(value) => {
                        merged.insert(key.clone(), value);
                    }
                    None => {
                        merged.remove(key);
                    }
                }
            }
            Ok(Some(Value::Object(merged)))
        }
        MergeStrategy::SetUnion => merge_set_union(base, overlay, pointer).map(Some),
        MergeStrategy::MergeById { id_key } => {
            merge_by_id(base, overlay, pointer, rules, &id_key).map(Some)
        }
    }
}

fn merge_set_union(base: &Value, overlay: &Value, pointer: &str) -> Result<Value, MergeError> {
    let (Some(base), Some(overlay)) = (base.as_array(), overlay.as_array()) else {
        return Err(MergeError::StrategyTypeMismatch {
            pointer: pointer.into(),
            expected: "array",
        });
    };
    let mut result = base.clone();
    let mut seen = BTreeSet::new();
    for value in base.iter().chain(overlay) {
        let key = canonical_json(value).map_err(|_| MergeError::Canonicalization)?;
        if seen.insert(key) && !base.contains(value) {
            result.push(value.clone());
        }
    }
    Ok(Value::Array(result))
}

fn merge_by_id(
    base: &Value,
    overlay: &Value,
    pointer: &str,
    rules: &MergeRules,
    id_key: &str,
) -> Result<Value, MergeError> {
    let (Some(base), Some(overlay)) = (base.as_array(), overlay.as_array()) else {
        return Err(MergeError::StrategyTypeMismatch {
            pointer: pointer.into(),
            expected: "array",
        });
    };
    let mut result = base.clone();
    let mut positions = BTreeMap::new();
    for (index, value) in result.iter().enumerate() {
        positions.insert(item_id(value, pointer, id_key)?, index);
    }
    for value in overlay {
        let id = item_id(value, pointer, id_key)?;
        if let Some(index) = positions.get(&id).copied() {
            let item_pointer = format!("{pointer}/{id}");
            result[index] =
                merge_value(&result[index], value, &item_pointer, rules)?.ok_or_else(|| {
                    MergeError::DeleteNotAllowed {
                        pointer: item_pointer.clone(),
                    }
                })?;
        } else {
            positions.insert(id, result.len());
            result.push(value.clone());
        }
    }
    Ok(Value::Array(result))
}

fn item_id(value: &Value, pointer: &str, id_key: &str) -> Result<String, MergeError> {
    value
        .as_object()
        .and_then(|object: &Map<String, Value>| object.get(id_key))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| MergeError::MissingItemId {
            pointer: pointer.into(),
            id_key: id_key.into(),
        })
}

fn is_delete(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 1 && object.get("$delete").and_then(Value::as_bool) == Some(true)
    })
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MergeError {
    #[error("delete is not allowed at {pointer}")]
    DeleteNotAllowed { pointer: String },
    #[error("merge strategy at {pointer} requires {expected}")]
    StrategyTypeMismatch {
        pointer: String,
        expected: &'static str,
    },
    #[error("merge-by-ID at {pointer} requires string key {id_key}")]
    MissingItemId { pointer: String, id_key: String },
    #[error("canonicalization failed during set union")]
    Canonicalization,
}

impl MergeError {
    pub fn pointer(&self) -> &str {
        match self {
            Self::DeleteNotAllowed { pointer }
            | Self::StrategyTypeMismatch { pointer, .. }
            | Self::MissingItemId { pointer, .. } => pointer,
            Self::Canonicalization => "",
        }
    }
}
