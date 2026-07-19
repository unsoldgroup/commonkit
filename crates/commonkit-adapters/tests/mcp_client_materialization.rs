use std::collections::BTreeMap;

use commonkit_adapters::{
    ArtifactStore, ExactProviderVersion, MaterializedState, NormalizedManagedPath,
    ProviderCapability, ProviderCapabilityResource, ProviderInputs, ResourceProvenance,
    materialize_mcp_client_state,
};
use commonkit_contracts::{Sha256Digest, StableId};
use tempfile::tempdir;

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn provider_state() -> MaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse("apm").unwrap(),
        ExactProviderVersion::parse("0.25.0").unwrap(),
        "commonkit.apm-provider.v1".into(),
        BTreeMap::from([("manifest".into(), digest('a'))]),
        vec!["agent-context".into()],
    )
    .unwrap();
    MaterializedState::finalize_with_capabilities(
        inputs.clone(),
        vec![],
        vec![],
        vec![],
        vec![ProviderCapabilityResource {
            capability: ProviderCapability::McpStreamableHttp {
                id: "docs".into(),
                name: "Docs".into(),
                enabled: true,
                url: "https://upstream.example/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "env:DOCS_TOKEN".into())]),
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "apm.yml:dependencies.mcp[docs]".into(),
            },
        }],
    )
    .unwrap()
}

#[test]
fn provider_mcp_becomes_content_addressed_claude_and_codex_relay_configs() {
    let root = tempdir().unwrap();
    let artifacts = ArtifactStore::open(root.path().join("artifacts")).unwrap();
    let managed_root = NormalizedManagedPath::parse("home").unwrap();
    let state = materialize_mcp_client_state(
        &[provider_state()],
        &artifacts,
        &managed_root,
        "http://127.0.0.1:3764/mcp",
    )
    .unwrap()
    .unwrap();

    state.verify().unwrap();
    assert_eq!(state.resources.len(), 2);
    let rendered = state
        .resources
        .iter()
        .map(|resource| {
            let commonkit_adapters::FilesystemIntent::File { path, content, .. } = &resource.intent
            else {
                panic!("client configuration must be a file")
            };
            (
                path.as_str().to_owned(),
                String::from_utf8(artifacts.load(content).unwrap()).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        rendered.keys().cloned().collect::<Vec<_>>(),
        vec!["home/.codex/config.toml", "home/.mcp.json"]
    );
    assert!(rendered["home/.mcp.json"].contains("http://127.0.0.1:3764/mcp"));
    assert!(rendered["home/.mcp.json"].contains("commonkit-relay"));
    assert!(rendered["home/.mcp.json"].contains("Bearer ${COMMONKIT_RELAY_TOKEN}"));
    assert!(rendered["home/.codex/config.toml"].contains("http://127.0.0.1:3764/mcp"));
    assert!(rendered["home/.codex/config.toml"].contains("mcp_servers.\"commonkit-relay\""));
    assert!(
        rendered["home/.codex/config.toml"]
            .contains("bearer_token_env_var = \"COMMONKIT_RELAY_TOKEN\"")
    );
    assert!(
        !rendered
            .values()
            .any(|content| content.contains("upstream.example"))
    );
    assert!(
        !rendered
            .values()
            .any(|content| content.contains("DOCS_TOKEN"))
    );
}
