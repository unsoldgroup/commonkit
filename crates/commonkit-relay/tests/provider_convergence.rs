use std::collections::BTreeMap;

use commonkit_relay::{
    McpDeclarationProvenance, PortableMcpDeclaration, PortableMcpResolution, RelayConvergenceError,
    ResolvedMcpDeclarations, converge_provider_mcp,
};

fn direct_server(id: &str) -> PortableMcpDeclaration {
    PortableMcpDeclaration {
        id: id.into(),
        name: format!("{id} server"),
        enabled: true,
        resolution: PortableMcpResolution::StreamableHttp {
            url: format!("https://{id}.example/mcp"),
            headers: BTreeMap::from([("Authorization".into(), "secret:DOCS_TOKEN".into())]),
        },
        provenance: McpDeclarationProvenance {
            provider_id: "apm".into(),
            source: "apm.lock.yaml#mcp_configs.docs".into(),
        },
    }
}

#[test]
fn provider_declarations_require_source_provenance() {
    let mut declaration = direct_server("docs");
    declaration.provenance.source.clear();

    let error = converge_provider_mcp(ResolvedMcpDeclarations {
        contract_version: "commonkit.resolved-mcp.v1".into(),
        declarations: vec![declaration],
    })
    .expect_err("missing provenance");

    assert_eq!(error, RelayConvergenceError::MissingProvenance);
}

#[test]
fn provider_declarations_reject_literal_header_credentials_without_leaking_them() {
    let mut declaration = direct_server("docs");
    let PortableMcpResolution::StreamableHttp { headers, .. } = &mut declaration.resolution else {
        unreachable!();
    };
    headers.insert("Authorization".into(), "Bearer private-value".into());

    let error = converge_provider_mcp(ResolvedMcpDeclarations {
        contract_version: "commonkit.resolved-mcp.v1".into(),
        declarations: vec![declaration],
    })
    .expect_err("literal credential");

    assert!(!error.to_string().contains("private-value"));
}

#[test]
fn unresolved_registry_entries_fail_with_the_documented_interface_gap() {
    let mut declaration = direct_server("registry");
    declaration.resolution = PortableMcpResolution::RegistryReference {
        reference: "io.modelcontextprotocol/example".into(),
    };

    let error = converge_provider_mcp(ResolvedMcpDeclarations {
        contract_version: "commonkit.resolved-mcp.v1".into(),
        declarations: vec![declaration],
    })
    .expect_err("registry interface");

    assert_eq!(
        error,
        RelayConvergenceError::ResolvedMcpInterfaceUnsupported
    );
    assert!(
        error
            .to_string()
            .contains("resolved_mcp_interface_unsupported")
    );
}

#[test]
fn provider_declarations_converge_on_one_stable_loopback_client_endpoint() {
    let resolved = ResolvedMcpDeclarations {
        contract_version: "commonkit.resolved-mcp.v1".into(),
        declarations: vec![direct_server("docs"), direct_server("search")],
    };

    let converged = converge_provider_mcp(resolved).expect("converged state");

    assert!(converged.requires_confirmation());
    assert_eq!(converged.relay.listen.host, "127.0.0.1");
    assert_eq!(converged.relay.servers.len(), 2);
    assert_eq!(converged.relay.servers[0].id.as_str(), "docs");
    assert_eq!(converged.relay.servers[1].id.as_str(), "search");
    assert_eq!(converged.client.name, "commonkit-relay");
    assert_eq!(converged.client.transport_type, "streamable_http");
    assert_eq!(converged.client.url, "http://127.0.0.1:3764/mcp");
}
