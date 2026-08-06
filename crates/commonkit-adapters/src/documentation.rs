use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use commonkit_contracts::{Sha256Digest, StableId, assert_no_embedded_secrets, digest_domain_json};
use futures_util::StreamExt;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use url::{Host, Url};

use crate::provider_sandbox::ProviderSandbox;

const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SECTIONS: usize = 2_000;
pub const DOCLING_VERSION: &str = "2.115.0";
const MAX_WEB_REDIRECTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RichExtractionLimits {
    pub cpu_seconds: u64,
    pub memory_mib: u64,
    pub wall_time_seconds: u64,
    pub max_input_bytes: usize,
    pub max_expanded_bytes: usize,
    pub max_pages: usize,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichExtractionPlan {
    pub executable: PathBuf,
    pub executable_digest: Sha256Digest,
    pub version: &'static str,
    pub network_enabled: bool,
    pub private_scratch: bool,
    pub limits: RichExtractionLimits,
}

impl RichExtractionPlan {
    pub fn docling(
        executable: PathBuf,
        executable_digest: Sha256Digest,
        limits: RichExtractionLimits,
    ) -> Result<Self, DocumentationIngestionError> {
        if !executable.is_absolute()
            || limits.cpu_seconds == 0
            || limits.memory_mib < 128
            || limits.wall_time_seconds == 0
            || limits.max_input_bytes == 0
            || limits.max_expanded_bytes < limits.max_input_bytes
            || limits.max_pages == 0
            || limits.max_output_bytes == 0
        {
            return Err(DocumentationIngestionError::InvalidExtractionPlan);
        }
        Ok(Self {
            executable,
            executable_digest,
            version: DOCLING_VERSION,
            network_enabled: false,
            private_scratch: true,
            limits,
        })
    }

    pub fn validate_observed_version(
        &self,
        observed: &str,
    ) -> Result<(), DocumentationIngestionError> {
        if observed == self.version {
            Ok(())
        } else {
            Err(DocumentationIngestionError::ExtractorVersionMismatch)
        }
    }
}

pub fn extract_rich_document(
    plan: &RichExtractionPlan,
    source_id: StableId,
    extension: &str,
    input: &[u8],
) -> Result<IngestionResult, DocumentationIngestionError> {
    if input.is_empty() || input.len() > plan.limits.max_input_bytes {
        return Err(DocumentationIngestionError::SourceTooLarge);
    }
    let extension = extension.trim_start_matches('.');
    if extension.is_empty()
        || extension.len() > 10
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(DocumentationIngestionError::InvalidSource);
    }
    verify_extractor(plan)?;
    let scratch = tempfile::tempdir().map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    commonkit_platform::ensure_private_path(
        scratch.path(),
        commonkit_platform::PrivatePathKind::Directory,
    )
    .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    let input_path = scratch.path().join(format!("source.{extension}"));
    std::fs::write(&input_path, input)
        .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    commonkit_platform::ensure_private_path(&input_path, commonkit_platform::PrivatePathKind::File)
        .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    let output_path = scratch.path().join("output");
    commonkit_platform::ensure_private_path(
        &output_path,
        commonkit_platform::PrivatePathKind::Directory,
    )
    .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;

    let version = sandboxed_extractor_output(plan, scratch.path(), ["--version"], &input_path)?;
    if !version.status.success() {
        return Err(DocumentationIngestionError::ExtractionFailed);
    }
    let observed = String::from_utf8(version.stdout)
        .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    if !observed
        .split_whitespace()
        .any(|token| token == plan.version)
    {
        return Err(DocumentationIngestionError::ExtractorVersionMismatch);
    }

    let output = sandboxed_extractor_output(
        plan,
        scratch.path(),
        [
            "--to",
            "md",
            "--output",
            output_path
                .to_str()
                .ok_or(DocumentationIngestionError::ExtractionFailed)?,
        ],
        &input_path,
    )?;
    if !output.status.success() {
        return Err(DocumentationIngestionError::ExtractionFailed);
    }
    let markdown_path = only_markdown_output(&output_path, plan.limits.max_expanded_bytes)?;
    let markdown =
        std::fs::read(markdown_path).map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    if markdown.len() > plan.limits.max_output_bytes {
        return Err(DocumentationIngestionError::ExtractionOutputTooLarge);
    }
    let original_hash = digest_domain_json("commonkit.documentation.original.v1", &input)
        .map_err(|_| DocumentationIngestionError::Hashing)?;
    let mut result = ingest_documentation(DocumentationSource::Markdown {
        source_id,
        path: format!("extracted.{extension}.md"),
        content: markdown,
    })?;
    result.original_hash = original_hash;
    Ok(result)
}

fn sandboxed_extractor_output<const N: usize>(
    plan: &RichExtractionPlan,
    scratch: &Path,
    arguments: [&str; N],
    input_path: &Path,
) -> Result<std::process::Output, DocumentationIngestionError> {
    let mut sandbox = ProviderSandbox::new(&plan.executable, scratch);
    sandbox
        .args(arguments)
        .readable_path(input_path)
        .writable_root(scratch)
        .limits(
            Duration::from_secs(plan.limits.wall_time_seconds),
            plan.limits.max_output_bytes,
        );
    if N > 1 {
        sandbox.arg(input_path);
    }
    sandbox
        .output()
        .map_err(|_| DocumentationIngestionError::ExtractionSandboxUnavailable)
}

fn verify_extractor(plan: &RichExtractionPlan) -> Result<(), DocumentationIngestionError> {
    use sha2::{Digest, Sha256};
    let metadata = std::fs::symlink_metadata(&plan.executable)
        .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DocumentationIngestionError::InvalidExtractionPlan);
    }
    let actual = Sha256Digest::parse(format!(
        "sha256:{:x}",
        Sha256::digest(
            std::fs::read(&plan.executable)
                .map_err(|_| DocumentationIngestionError::ExtractionFailed)?
        )
    ))
    .map_err(|_| DocumentationIngestionError::Hashing)?;
    if actual != plan.executable_digest {
        return Err(DocumentationIngestionError::ExtractorDigestMismatch);
    }
    Ok(())
}

fn only_markdown_output(
    root: &Path,
    max_expanded_bytes: usize,
) -> Result<PathBuf, DocumentationIngestionError> {
    let mut markdown = Vec::new();
    let mut total = 0_usize;
    for entry in
        std::fs::read_dir(root).map_err(|_| DocumentationIngestionError::ExtractionFailed)?
    {
        let entry = entry.map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| DocumentationIngestionError::ExtractionFailed)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DocumentationIngestionError::ExtractionFailed);
        }
        total = total.saturating_add(metadata.len() as usize);
        if total > max_expanded_bytes {
            return Err(DocumentationIngestionError::ExtractionOutputTooLarge);
        }
        if entry.path().extension().and_then(|value| value.to_str()) == Some("md") {
            markdown.push(entry.path());
        }
    }
    if markdown.len() != 1 {
        return Err(DocumentationIngestionError::ExtractionFailed);
    }
    Ok(markdown.remove(0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentationSource {
    Markdown {
        source_id: StableId,
        path: String,
        content: Vec<u8>,
    },
    WebSnapshot {
        source_id: StableId,
        url: String,
        content_type: String,
        content: Vec<u8>,
    },
}

/// Fetches one pinned HTTPS snapshot while defending every redirect hop
/// against DNS rebinding and private-network access.
pub async fn fetch_web_snapshot(
    source_id: StableId,
    initial_url: &str,
) -> Result<DocumentationSource, DocumentationIngestionError> {
    let mut current =
        Url::parse(initial_url).map_err(|_| DocumentationIngestionError::UnsafeWebSource)?;
    for redirect_count in 0..=MAX_WEB_REDIRECTS {
        validate_web_url(&current)?;
        let host = current
            .host_str()
            .ok_or(DocumentationIngestionError::UnsafeWebSource)?;
        let port = current
            .port_or_known_default()
            .ok_or(DocumentationIngestionError::UnsafeWebSource)?;
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| DocumentationIngestionError::WebFetchFailed)?
            .collect::<Vec<_>>();
        validate_resolved_web_addresses(&addresses)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| DocumentationIngestionError::WebFetchFailed)?;
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(|_| DocumentationIngestionError::WebFetchFailed)?;
        if response.status().is_redirection() {
            if redirect_count == MAX_WEB_REDIRECTS {
                return Err(DocumentationIngestionError::TooManyRedirects);
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(DocumentationIngestionError::UnsafeWebSource)?;
            current = current
                .join(location)
                .map_err(|_| DocumentationIngestionError::UnsafeWebSource)?;
            continue;
        }
        if !response.status().is_success() {
            return Err(DocumentationIngestionError::WebFetchFailed);
        }
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > MAX_SOURCE_BYTES)
        {
            return Err(DocumentationIngestionError::SourceTooLarge);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .ok_or(DocumentationIngestionError::UnsafeWebSource)?
            .to_owned();
        validate_web_source(current.as_str(), &content_type)?;
        let mut content = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| DocumentationIngestionError::WebFetchFailed)?;
            if content.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
                return Err(DocumentationIngestionError::SourceTooLarge);
            }
            content.extend_from_slice(&chunk);
        }
        return Ok(DocumentationSource::WebSnapshot {
            source_id,
            url: current.to_string(),
            content_type,
            content,
        });
    }
    Err(DocumentationIngestionError::TooManyRedirects)
}

pub fn validate_resolved_web_addresses(
    addresses: &[SocketAddr],
) -> Result<(), DocumentationIngestionError> {
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(DocumentationIngestionError::UnsafeWebSource);
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.is_multicast()
                || octets[0] == 0
                || octets[0] >= 224
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 198 && (18..=19).contains(&octets[1])))
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first == 0x2001 && ip.segments()[1] == 0x0db8))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionDisposition {
    ReviewRequired,
    Quarantined,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectDocumentationRole {
    Viewer,
    Editor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentationSectionCandidate {
    pub id: StableId,
    pub heading: String,
    pub normalized_text: String,
    pub content_hash: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IngestionResult {
    pub source_id: StableId,
    pub original_hash: Sha256Digest,
    pub normalized_hash: Option<Sha256Digest>,
    pub sections: Vec<DocumentationSectionCandidate>,
    pub disposition: IngestionDisposition,
    pub active: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectContextProposal {
    pub path: String,
    pub source_digest: Sha256Digest,
    pub active: bool,
}

pub fn discover_project_context(paths: &[String]) -> Vec<ProjectContextProposal> {
    let mut paths = paths
        .iter()
        .filter(|path| {
            matches!(path.as_str(), "AGENTS.md" | "CONTEXT.md" | "README.md")
                || (path.starts_with("docs/") && path.ends_with(".md"))
        })
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let source_digest =
                digest_domain_json("commonkit.documentation.discovery.v1", &path).ok()?;
            Some(ProjectContextProposal {
                path,
                source_digest,
                active: false,
            })
        })
        .collect()
}

pub fn ingest_documentation(
    source: DocumentationSource,
) -> Result<IngestionResult, DocumentationIngestionError> {
    let (source_id, content) = match source {
        DocumentationSource::Markdown {
            source_id,
            path,
            content,
        } => {
            if path.trim().is_empty() {
                return Err(DocumentationIngestionError::InvalidSource);
            }
            (source_id, content)
        }
        DocumentationSource::WebSnapshot {
            source_id,
            url,
            content_type,
            content,
        } => {
            validate_web_source(&url, &content_type)?;
            (source_id, content)
        }
    };
    if content.len() > MAX_SOURCE_BYTES {
        return Err(DocumentationIngestionError::SourceTooLarge);
    }
    let original_hash = digest_domain_json("commonkit.documentation.original.v1", &content)
        .map_err(|_| DocumentationIngestionError::Hashing)?;
    let text = String::from_utf8(content).map_err(|_| DocumentationIngestionError::NonUtf8)?;
    if assert_no_embedded_secrets(&json!({ "content": text })).is_err() {
        return Ok(IngestionResult {
            source_id,
            original_hash,
            normalized_hash: None,
            sections: Vec::new(),
            disposition: IngestionDisposition::Quarantined,
            active: false,
            warning: Some("suspected secret at /content".into()),
        });
    }
    let normalized = normalize_markdown(&text);
    let normalized_hash = digest_domain_json("commonkit.documentation.normalized.v1", &normalized)
        .map_err(|_| DocumentationIngestionError::Hashing)?;
    let sections = split_sections(&source_id, &normalized)?;
    Ok(IngestionResult {
        source_id,
        original_hash,
        normalized_hash: Some(normalized_hash),
        sections,
        disposition: IngestionDisposition::ReviewRequired,
        active: false,
        warning: None,
    })
}

pub fn publish_documentation(
    mut candidate: IngestionResult,
    role: ProjectDocumentationRole,
) -> Result<IngestionResult, DocumentationIngestionError> {
    if role != ProjectDocumentationRole::Editor {
        return Err(DocumentationIngestionError::ProjectEditorRequired);
    }
    if candidate.disposition == IngestionDisposition::Quarantined {
        return Err(DocumentationIngestionError::QuarantinedSource);
    }
    candidate.active = true;
    candidate.disposition = IngestionDisposition::Published;
    Ok(candidate)
}

fn normalize_markdown(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    output
}

fn split_sections(
    source_id: &StableId,
    markdown: &str,
) -> Result<Vec<DocumentationSectionCandidate>, DocumentationIngestionError> {
    let mut raw = Vec::<(String, String)>::new();
    let mut heading = None::<String>;
    let mut body = String::new();
    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("# ").or_else(|| line.strip_prefix("## ")) {
            if let Some(previous) = heading.replace(title.trim().to_owned()) {
                raw.push((previous, finish_body(&body)));
                body.clear();
            }
        } else if heading.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(previous) = heading {
        raw.push((previous, finish_body(&body)));
    }
    if raw.len() > MAX_SECTIONS {
        return Err(DocumentationIngestionError::TooManySections);
    }
    raw.into_iter()
        .enumerate()
        .map(|(index, (heading, normalized_text))| {
            let content_hash = digest_domain_json(
                "commonkit.documentation.section.v1",
                &(source_id, index, &heading, &normalized_text),
            )
            .map_err(|_| DocumentationIngestionError::Hashing)?;
            Ok(DocumentationSectionCandidate {
                id: StableId::parse(format!("section-{}", index + 1))
                    .map_err(|_| DocumentationIngestionError::Hashing)?,
                heading,
                normalized_text,
                content_hash,
            })
        })
        .collect()
}

fn finish_body(body: &str) -> String {
    format!("{}\n", body.trim_matches('\n'))
}

fn validate_web_source(url: &str, content_type: &str) -> Result<(), DocumentationIngestionError> {
    let url = Url::parse(url).map_err(|_| DocumentationIngestionError::UnsafeWebSource)?;
    validate_web_url(&url)?;
    if !matches!(content_type, "text/markdown" | "text/plain" | "text/html") {
        return Err(DocumentationIngestionError::UnsafeWebSource);
    }
    Ok(())
}

fn validate_web_url(url: &Url) -> Result<(), DocumentationIngestionError> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(DocumentationIngestionError::UnsafeWebSource);
    }
    match url.host() {
        Some(Host::Domain(domain))
            if !domain.eq_ignore_ascii_case("localhost") && !domain.ends_with(".localhost") =>
        {
            Ok(())
        }
        _ => Err(DocumentationIngestionError::UnsafeWebSource),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DocumentationIngestionError {
    #[error("documentation source is invalid")]
    InvalidSource,
    #[error("documentation source exceeds the size limit")]
    SourceTooLarge,
    #[error("documentation source is not UTF-8")]
    NonUtf8,
    #[error("web documentation source is unsafe")]
    UnsafeWebSource,
    #[error("web documentation source could not be fetched")]
    WebFetchFailed,
    #[error("web documentation source exceeded the redirect limit")]
    TooManyRedirects,
    #[error("documentation source has too many sections")]
    TooManySections,
    #[error("documentation digest could not be computed")]
    Hashing,
    #[error("rich-document extraction plan is invalid")]
    InvalidExtractionPlan,
    #[error("rich-document extractor version does not match the pinned version")]
    ExtractorVersionMismatch,
    #[error("rich-document extractor digest does not match the reviewed executable")]
    ExtractorDigestMismatch,
    #[error("rich-document extraction failed")]
    ExtractionFailed,
    #[error("rich-document extraction output exceeded its bound")]
    ExtractionOutputTooLarge,
    #[error("rich-document isolation is unavailable on this platform")]
    ExtractionSandboxUnavailable,
    #[error("project-editor role is required to publish documentation")]
    ProjectEditorRequired,
    #[error("quarantined documentation cannot be published")]
    QuarantinedSource,
}
