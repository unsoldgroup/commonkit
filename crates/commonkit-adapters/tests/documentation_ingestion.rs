use commonkit_adapters::{
    DOCLING_VERSION, DocumentationIngestionError, DocumentationSource, IngestionDisposition,
    ProjectDocumentationRole, RichExtractionLimits, RichExtractionPlan, discover_project_context,
    extract_rich_document, ingest_documentation, publish_documentation,
    validate_resolved_web_addresses,
};
use commonkit_contracts::{Sha256Digest, StableId};
use std::net::SocketAddr;
use std::path::PathBuf;

#[test]
fn markdown_is_normalized_into_deterministic_review_sections() {
    let source = DocumentationSource::Markdown {
        source_id: StableId::parse("handbook").unwrap(),
        path: "docs/handbook.md".into(),
        content: b"# Team\r\n\r\nWelcome.  \r\n\r\n## Decisions\r\nWrite ADRs.\r\n".to_vec(),
    };

    let first = ingest_documentation(source.clone()).unwrap();
    let second = ingest_documentation(source).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.disposition, IngestionDisposition::ReviewRequired);
    assert_eq!(first.sections.len(), 2);
    assert_eq!(first.sections[0].heading, "Team");
    assert_eq!(first.sections[1].heading, "Decisions");
    assert_eq!(first.sections[1].normalized_text, "Write ADRs.\n");
    assert!(!first.active);
}

#[test]
fn suspected_secrets_are_quarantined_without_echoing_them() {
    let source = DocumentationSource::Markdown {
        source_id: StableId::parse("runbook").unwrap(),
        path: "RUNBOOK.md".into(),
        content: b"# Deploy\nAPI_TOKEN=super-secret-token-value\n".to_vec(),
    };

    let result = ingest_documentation(source).unwrap();
    assert_eq!(result.disposition, IngestionDisposition::Quarantined);
    assert!(result.sections.is_empty());
    let diagnostic = result.warning.unwrap();
    assert!(!diagnostic.contains("super-secret-token-value"));
    assert_eq!(diagnostic, "suspected secret at /content");
}

#[test]
fn web_snapshots_require_safe_https_origins() {
    for url in [
        "http://example.com/handbook",
        "https://127.0.0.1/private",
        "https://169.254.169.254/latest/meta-data",
        "https://user:password@example.com/",
    ] {
        let error = ingest_documentation(DocumentationSource::WebSnapshot {
            source_id: StableId::parse("web-handbook").unwrap(),
            url: url.into(),
            content_type: "text/markdown".into(),
            content: b"# Safe\nText\n".to_vec(),
        })
        .unwrap_err();
        assert_eq!(error, DocumentationIngestionError::UnsafeWebSource);
    }
}

#[test]
fn web_fetch_dns_results_reject_any_private_or_special_address() {
    assert!(
        validate_resolved_web_addresses(&["93.184.216.34:443".parse::<SocketAddr>().unwrap()])
            .is_ok()
    );
    for address in [
        "127.0.0.1:443",
        "10.0.0.1:443",
        "169.254.169.254:443",
        "100.64.0.1:443",
        "[::1]:443",
        "[fc00::1]:443",
        "[fe80::1]:443",
    ] {
        assert!(
            validate_resolved_web_addresses(&[address.parse::<SocketAddr>().unwrap()]).is_err(),
            "{address} must be rejected"
        );
    }
}

#[test]
fn rich_extraction_is_pinned_offline_and_resource_bounded() {
    let plan = RichExtractionPlan::docling(
        PathBuf::from("/opt/commonkit/docling"),
        Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
        RichExtractionLimits {
            cpu_seconds: 60,
            memory_mib: 1_024,
            wall_time_seconds: 120,
            max_input_bytes: 16 * 1024 * 1024,
            max_expanded_bytes: 64 * 1024 * 1024,
            max_pages: 500,
            max_output_bytes: 32 * 1024 * 1024,
        },
    )
    .unwrap();
    assert_eq!(plan.version, DOCLING_VERSION);
    assert!(!plan.network_enabled);
    assert!(plan.private_scratch);
    assert!(plan.validate_observed_version("2.115.0").is_ok());
    assert!(plan.validate_observed_version("2.116.0").is_err());
}

#[test]
fn rich_extraction_refuses_an_unreviewed_executable_before_launch() {
    let plan = RichExtractionPlan::docling(
        PathBuf::from("/bin/echo"),
        Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
        RichExtractionLimits {
            cpu_seconds: 1,
            memory_mib: 128,
            wall_time_seconds: 1,
            max_input_bytes: 1024,
            max_expanded_bytes: 2048,
            max_pages: 1,
            max_output_bytes: 1024,
        },
    )
    .unwrap();
    assert_eq!(
        extract_rich_document(&plan, StableId::parse("rich-1").unwrap(), "pdf", b"pdf")
            .unwrap_err(),
        DocumentationIngestionError::ExtractorDigestMismatch
    );
}

#[test]
fn only_project_editors_can_publish_reviewed_candidates() {
    let candidate = ingest_documentation(DocumentationSource::Markdown {
        source_id: StableId::parse("handbook").unwrap(),
        path: "README.md".into(),
        content: b"# Project\nReviewed context.\n".to_vec(),
    })
    .unwrap();
    assert_eq!(
        publish_documentation(candidate.clone(), ProjectDocumentationRole::Viewer).unwrap_err(),
        DocumentationIngestionError::ProjectEditorRequired
    );
    let published = publish_documentation(candidate, ProjectDocumentationRole::Editor).unwrap();
    assert!(published.active);
    assert_eq!(published.disposition, IngestionDisposition::Published);
}

#[test]
fn project_discovery_proposes_only_context_bearing_files_in_stable_order() {
    let proposals = discover_project_context(&[
        "src/main.rs".into(),
        "docs/operations.md".into(),
        "README.md".into(),
        "AGENTS.md".into(),
        "docs/image.png".into(),
    ]);
    assert_eq!(
        proposals
            .iter()
            .map(|proposal| proposal.path.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md", "README.md", "docs/operations.md"]
    );
    assert!(proposals.iter().all(|proposal| !proposal.active));
}
