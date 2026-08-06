use std::collections::BTreeMap;

use commonkit_adapters::{
    ExactProviderVersion, MaterializedState, ProviderCapability, ProviderCapabilityResource,
    ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_relay::{DEFAULT_PORT, converge_provider_mcp};
use commonkit_service::{ProviderMcpError, resolved_mcp_from_materialized};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn state(provider: &str, server: &str, seed: char) -> MaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse(provider).unwrap(),
        ExactProviderVersion::parse("0.25.0").unwrap(),
        "commonkit.apm-provider.v1".into(),
        BTreeMap::from([
            ("manifest".into(), digest(seed)),
            ("lockfile".into(), digest('b')),
        ]),
        vec!["agent-context".into()],
    )
    .unwrap();
    let provenance = ResourceProvenance {
        provider_id: inputs.provider_id.clone(),
        provider_version: inputs.provider_version.to_string(),
        input_digest: inputs.input_set_digest.clone(),
        source: format!("apm.yml:dependencies.mcp[{server}] -> .mcp.json:mcpServers.{server}"),
    };
    MaterializedState::finalize_with_capabilities(
        inputs,
        vec![],
        vec![],
        vec![],
        vec![ProviderCapabilityResource {
            capability: ProviderCapability::McpStreamableHttp {
                id: server.into(),
                name: server.into(),
                enabled: true,
                url: format!("https://{server}.example/mcp"),
                headers: BTreeMap::from([("Authorization".into(), "env:MCP_TOKEN".into())]),
            },
            provenance,
        }],
    )
    .unwrap()
}

#[test]
fn apm_capability_flows_to_relay_and_stable_client_config_with_consent() {
    let materialized = state("apm", "docs", 'a');
    let resolved = resolved_mcp_from_materialized(&[materialized]).unwrap();
    let converged = converge_provider_mcp(resolved).unwrap();

    assert!(converged.requires_confirmation());
    assert_eq!(converged.relay.servers[0].id.as_str(), "docs");
    assert_eq!(
        converged.client.url,
        format!("http://127.0.0.1:{DEFAULT_PORT}/mcp")
    );
    assert_eq!(
        converged.provenance.values().next().unwrap().provider_id,
        "apm"
    );
}

#[test]
fn provider_ownership_collision_fails_before_relay_planning() {
    let error =
        resolved_mcp_from_materialized(&[state("apm", "docs", 'a'), state("native", "docs", 'c')])
            .unwrap_err();
    assert_eq!(error, ProviderMcpError::OwnershipCollision);
}

#[test]
fn manifest_input_change_changes_durable_materialization_binding() {
    let before = state("apm", "docs", 'a');
    let after = state("apm", "docs", 'c');
    assert_ne!(
        before.inputs.input_set_digest,
        after.inputs.input_set_digest
    );
    assert_ne!(before.digest, after.digest);
}

#[test]
fn tampered_materialized_capability_is_rejected_before_relay_planning() {
    let mut materialized = state("apm", "docs", 'a');
    let ProviderCapability::McpStreamableHttp { name, .. } =
        &mut materialized.capabilities[0].capability;
    *name = "tampered".into();
    assert_eq!(
        resolved_mcp_from_materialized(&[materialized]).unwrap_err(),
        ProviderMcpError::InvalidMaterialization
    );
}
