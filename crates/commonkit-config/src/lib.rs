//! Layer loading and composition.

use std::collections::{BTreeMap, BTreeSet};

use commonkit_contracts::{
    CommonKitLock, ContractError, Contribution, LayerDocument, LayerKind, LockedLayer,
    MergeOperation, ProvenanceTrace, SCHEMA_VERSION, SchemaVersion, Sha256Digest, StableId,
    StyleguideBinding, StyleguideDescriptor, StyleguideSelection, TraceEntry, V1_LAYER_SPEC_FIELDS,
    canonical_json, digest_domain_json, digest_json,
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
            validate_v1_spec(&layer.spec)?;
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

fn validate_v1_spec(spec: &Value) -> Result<(), LayerSetError> {
    let object = spec.as_object().ok_or(LayerSetError::SpecMustBeObject)?;
    if let Some(field) = object
        .keys()
        .find(|field| !V1_LAYER_SPEC_FIELDS.contains(&field.as_str()))
    {
        if FORMER_V1_LAYER_SPEC_FIELDS.contains(&field.as_str()) {
            return Err(LayerSetError::UnsupportedLayerSpecField {
                field: field.clone(),
            });
        }
        return Err(LayerSetError::UnknownSpecField {
            field: field.clone(),
        });
    }
    if object.get("files").is_some_and(|value| !value.is_array()) {
        return Err(LayerSetError::InvalidSpecFieldType {
            field: "files".into(),
            expected: "array",
        });
    }
    if object
        .get("securityPolicy")
        .is_some_and(|value| !value.is_object())
    {
        return Err(LayerSetError::InvalidSpecFieldType {
            field: "securityPolicy".into(),
            expected: "object",
        });
    }
    if object
        .get("capabilities")
        .is_some_and(|value| !value.is_object())
    {
        return Err(LayerSetError::InvalidSpecFieldType {
            field: "capabilities".into(),
            expected: "object",
        });
    }
    if let Some(styleguide) = object
        .get("capabilities")
        .and_then(Value::as_object)
        .and_then(|capabilities| capabilities.get("styleguide"))
    {
        serde_json::from_value::<StyleguideSelection>(styleguide.clone())
            .map_err(|error| LayerSetError::InvalidStyleguideSelection(error.to_string()))?;
    }
    Ok(())
}

const FORMER_V1_LAYER_SPEC_FIELDS: &[&str] = &[
    "adapters",
    "arguments",
    "credentials",
    "databases",
    "denials",
    "hooks",
    "plugins",
    "relay",
    "requirements",
    "schedules",
    "services",
    "settings",
    "snapshots",
    "targets",
    "theme",
];

pub struct CompositionResult {
    pub spec: Value,
    pub spec_digest: Sha256Digest,
    pub trace: ProvenanceTrace,
    pub lock: CommonKitLock,
}

/// Computes the canonical digest of a layer without including the digest field
/// itself. Formatting differences in the source file do not change this value;
/// every typed semantic field, including source path and revision, does.
pub fn layer_content_digest(layer: &LayerDocument) -> Result<Sha256Digest, ContractError> {
    let mut value = serde_json::to_value(layer).map_err(|_| ContractError::Canonicalization)?;
    value
        .get_mut("source")
        .and_then(Value::as_object_mut)
        .and_then(|source| source.remove("contentDigest"))
        .ok_or(ContractError::Canonicalization)?;
    digest_domain_json("commonkit.layer-content.v1", &value)
}

pub fn validate_layer_content_digest(layer: &LayerDocument) -> Result<(), LayerIntegrityError> {
    let actual = layer_content_digest(layer)?;
    if actual != layer.source.content_digest {
        return Err(LayerIntegrityError::ContentDigestMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum LayerIntegrityError {
    #[error("layer content digest does not match canonical content")]
    ContentDigestMismatch,
    #[error(transparent)]
    Contract(#[from] ContractError),
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
                let governing_rules = rules.governing_rules(&pointer);
                (
                    pointer,
                    TraceEntry {
                        winner,
                        contributions,
                        governing_rules,
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
    #[error("layer spec must be an object")]
    SpecMustBeObject,
    #[error("unknown CommonKit v1 spec field: {field}")]
    UnknownSpecField { field: String },
    #[error(
        "CommonKit v1 layer declaration {field} is not supported; this does not mean the feature is unavailable, only that it is configured outside layers"
    )]
    UnsupportedLayerSpecField { field: String },
    #[error("CommonKit v1 spec field {field} must be {expected}")]
    InvalidSpecFieldType {
        field: String,
        expected: &'static str,
    },
    #[error("invalid styleguide selection: {0}")]
    InvalidStyleguideSelection(String),
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

    fn governing_rules(&self, pointer: &str) -> Vec<String> {
        let mut governing = Vec::new();
        if let Some(strategy) = self.strategies.get(pointer) {
            governing.push(format!(
                "schema:{pointer}:{}",
                match strategy {
                    MergeStrategy::Replace => "replace".to_owned(),
                    MergeStrategy::RecursiveMap => "recursive_map".to_owned(),
                    MergeStrategy::SetUnion => "set_union".to_owned(),
                    MergeStrategy::MergeById { id_key } => {
                        format!("merge_by_id({id_key})")
                    }
                }
            ));
        }
        if pointer == "/securityPolicy" || pointer.starts_with("/securityPolicy/") {
            governing.push("organization-security-floor:non-overridable".to_owned());
        }
        governing
    }
}

/// The merge contract for the version-one CommonKit schema.
///
/// Keeping this constructor in the composition crate gives the CLI and service
/// one authoritative rule set instead of letting entry points infer collection
/// behavior independently.
pub fn v1_merge_rules() -> MergeRules {
    MergeRules::new()
        .with_strategy("/securityPolicy/deniedPaths", MergeStrategy::SetUnion)
        .with_strategy("/capabilities/styleguide", MergeStrategy::Replace)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleguidePolicy {
    pub denied: bool,
    pub pinned_skill_id: Option<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStyleguide {
    pub selection: StyleguideSelection,
    pub descriptor: StyleguideDescriptor,
    /// Canonical normalized-state and plan-provenance binding. Callers include
    /// `binding.digest()` in their provider-input digest before planning.
    pub binding: StyleguideBinding,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StyleguideResolutionError {
    #[error("organization policy denies styleguides")]
    DeniedByOrganization,
    #[error("organization policy pins styleguide {expected}, but the loadout selected {actual}")]
    OrganizationPinMismatch {
        expected: StableId,
        actual: StableId,
    },
    #[error("selected styleguide skill is not exported by staged APM output: {0}")]
    MissingExport(StableId),
    #[error("staged APM output contains multiple active styleguide exports")]
    AmbiguousExports,
    #[error("selected styleguide has no descriptor: {0}")]
    MissingDescriptor(StableId),
    #[error("styleguide descriptor exports {descriptor}, but the loadout selected {selection}")]
    DescriptorExportMismatch {
        selection: StableId,
        descriptor: StableId,
    },
    #[error(transparent)]
    InvalidDescriptor(#[from] ContractError),
}

pub fn resolve_styleguide_selection(
    selection: Option<&StyleguideSelection>,
    descriptors: &BTreeMap<StableId, StyleguideDescriptor>,
    staged_styleguide_exports: &BTreeSet<StableId>,
    policy: &StyleguidePolicy,
) -> Result<Option<ResolvedStyleguide>, StyleguideResolutionError> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    if policy.denied {
        return Err(StyleguideResolutionError::DeniedByOrganization);
    }
    if let Some(expected) = &policy.pinned_skill_id
        && expected != &selection.skill_id
    {
        return Err(StyleguideResolutionError::OrganizationPinMismatch {
            expected: expected.clone(),
            actual: selection.skill_id.clone(),
        });
    }
    if staged_styleguide_exports.len() > 1 {
        return Err(StyleguideResolutionError::AmbiguousExports);
    }
    if !staged_styleguide_exports.contains(&selection.skill_id) {
        return Err(StyleguideResolutionError::MissingExport(
            selection.skill_id.clone(),
        ));
    }
    let descriptor = descriptors
        .get(&selection.skill_id)
        .ok_or_else(|| StyleguideResolutionError::MissingDescriptor(selection.skill_id.clone()))?
        .clone();
    if descriptor.exported_skill != selection.skill_id {
        return Err(StyleguideResolutionError::DescriptorExportMismatch {
            selection: selection.skill_id.clone(),
            descriptor: descriptor.exported_skill,
        });
    }
    let descriptor_digest = descriptor.digest()?;
    let binding = StyleguideBinding {
        selection: selection.clone(),
        descriptor_digest,
        package_manifest_digest: descriptor.package.manifest_digest.clone(),
        package_lock_digest: descriptor.package.lock_digest.clone(),
        evaluation_suite_digest: descriptor.evaluation_suite_digest.clone(),
        retention_map_digest: descriptor.retention_map_digest.clone(),
    };
    Ok(Some(ResolvedStyleguide {
        selection: selection.clone(),
        descriptor,
        binding,
    }))
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
    if base.is_null() {
        return Ok(Some(overlay.clone()));
    }

    match rules.strategy(pointer, base, overlay) {
        MergeStrategy::Replace => Ok(Some(overlay.clone())),
        MergeStrategy::RecursiveMap => {
            let Some(base) = base.as_object() else {
                return Err(MergeError::StrategyTypeMismatch {
                    pointer: pointer.into(),
                    expected: "object",
                });
            };
            let Some(overlay) = overlay.as_object() else {
                return Err(MergeError::StrategyTypeMismatch {
                    pointer: pointer.into(),
                    expected: "object",
                });
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
        let id = item_id(value, pointer, id_key)?;
        if positions.insert(id.clone(), index).is_some() {
            return Err(MergeError::DuplicateItemId {
                pointer: pointer.into(),
                id,
            });
        }
    }
    let mut overlay_ids = BTreeSet::new();
    for value in overlay {
        let id = item_id(value, pointer, id_key)?;
        if !overlay_ids.insert(id.clone()) {
            return Err(MergeError::DuplicateItemId {
                pointer: pointer.into(),
                id,
            });
        }
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
    #[error("merge-by-ID at {pointer} contains duplicate ID {id}")]
    DuplicateItemId { pointer: String, id: String },
    #[error("canonicalization failed during set union")]
    Canonicalization,
}

impl MergeError {
    pub fn pointer(&self) -> &str {
        match self {
            Self::DeleteNotAllowed { pointer }
            | Self::StrategyTypeMismatch { pointer, .. }
            | Self::MissingItemId { pointer, .. }
            | Self::DuplicateItemId { pointer, .. } => pointer,
            Self::Canonicalization => "",
        }
    }
}
