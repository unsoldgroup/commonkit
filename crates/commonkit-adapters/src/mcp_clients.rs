use std::collections::BTreeMap;

use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};
use serde::Serialize;
use thiserror::Error;

use crate::{
    ArtifactError, ArtifactStore, ContentSensitivity, ExactProviderVersion, FilesystemIntent,
    MaterializedState, NormalizedManagedPath, NormalizedResource, ProviderCapability,
    ProviderContractError, ProviderInputs, ResourceProvenance,
};

const CLIENT_PROVIDER_ID: &str = "commonkit-relay-client";
const CLIENT_PROVIDER_VERSION: &str = "2.0.0";

#[derive(Debug, Error)]
pub enum McpClientMaterializationError {
    #[error("relay client endpoint must be an explicit target-local loopback HTTP endpoint")]
    UnsafeEndpoint,
    #[error("provider output failed integrity verification")]
    InvalidProviderState,
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Contract(#[from] ProviderContractError),
    #[error(transparent)]
    Resource(#[from] crate::ResourceError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Digest(#[from] commonkit_contracts::ContractError),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityBinding<'a> {
    state_digest: &'a Sha256Digest,
    provider_id: &'a StableId,
    capability: &'a ProviderCapability,
    source: &'a str,
}

#[derive(Serialize)]
struct ClaudeClient {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<&'static str, ClaudeServer>,
}

#[derive(Serialize)]
struct ClaudeServer {
    #[serde(rename = "type")]
    transport_type: &'static str,
    command: &'static str,
    args: [&'static str; 1],
}

/// Converts verified provider MCP capabilities into portable client files.
///
/// These resources launch CommonKit's local stdio bridge. The bridge discovers
/// the target-local relay and reads its private installation token at runtime;
/// no endpoint credentials enter portable client files.
/// The returned state can be fed directly to the normal provider plan, so its
/// artifacts, ownership, receipt, verification, and rollback are durable.
pub fn materialize_mcp_client_state(
    states: &[MaterializedState],
    artifacts: &ArtifactStore,
    managed_root: &NormalizedManagedPath,
    relay_endpoint: &str,
) -> Result<Option<MaterializedState>, McpClientMaterializationError> {
    validate_target_local_endpoint(relay_endpoint)?;
    let mut bindings = Vec::new();
    for state in states {
        state
            .verify()
            .map_err(|_| McpClientMaterializationError::InvalidProviderState)?;
        for resource in &state.capabilities {
            bindings.push(CapabilityBinding {
                state_digest: &state.digest,
                provider_id: &resource.provenance.provider_id,
                capability: &resource.capability,
                source: &resource.provenance.source,
            });
        }
    }
    if bindings.is_empty() {
        return Ok(None);
    }
    bindings.sort_by(|left, right| {
        (left.provider_id, left.source, left.capability).cmp(&(
            right.provider_id,
            right.source,
            right.capability,
        ))
    });
    let capability_digest =
        digest_domain_json("commonkit.relay-client-capability-set.v1", &bindings)?;
    let endpoint_digest =
        digest_domain_json("commonkit.relay-client-endpoint.v1", &relay_endpoint)?;
    let inputs = ProviderInputs::new(
        StableId::parse(CLIENT_PROVIDER_ID)?,
        ExactProviderVersion::parse(CLIENT_PROVIDER_VERSION)?,
        "commonkit.relay-client-provider.v1".into(),
        BTreeMap::from([
            ("capabilities".into(), capability_digest),
            ("relayEndpoint".into(), endpoint_digest),
        ]),
        vec!["claude-mcp-client".into(), "codex-mcp-client".into()],
    )?;
    let claude = serde_json::to_vec(&ClaudeClient {
        mcp_servers: BTreeMap::from([(
            "commonkit-relay",
            ClaudeServer {
                transport_type: "stdio",
                command: "commonkit",
                args: ["relay-client"],
            },
        )]),
    })?;
    let codex =
        b"[mcp_servers.\"commonkit-relay\"]\ncommand = \"commonkit\"\nargs = [\"relay-client\"]\n"
            .to_vec();
    let resources = [
        (".mcp.json", claude, "provider-mcp -> claude relay client"),
        (
            ".codex/config.toml",
            codex,
            "provider-mcp -> codex relay client",
        ),
    ]
    .into_iter()
    .map(|(relative, bytes, source)| {
        let content = artifacts.put(&bytes, ContentSensitivity::Portable)?;
        Ok(NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse(format!(
                    "{}/{relative}",
                    managed_root.as_str()
                ))?,
                content,
                mode: None,
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: source.into(),
            },
        })
    })
    .collect::<Result<Vec<_>, McpClientMaterializationError>>()?;
    MaterializedState::finalize(inputs, resources, Vec::new(), Vec::new())
        .map(Some)
        .map_err(Into::into)
}

fn validate_target_local_endpoint(endpoint: &str) -> Result<(), McpClientMaterializationError> {
    let Some(port_and_path) = endpoint.strip_prefix("http://127.0.0.1:") else {
        return Err(McpClientMaterializationError::UnsafeEndpoint);
    };
    let Some(port) = port_and_path.strip_suffix("/mcp") else {
        return Err(McpClientMaterializationError::UnsafeEndpoint);
    };
    if port.parse::<u16>().ok().filter(|port| *port > 0).is_none() {
        return Err(McpClientMaterializationError::UnsafeEndpoint);
    }
    Ok(())
}
