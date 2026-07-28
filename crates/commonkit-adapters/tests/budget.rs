use commonkit_adapters::{
    ArtifactStore, BYTES_PER_TOKEN, ContentSensitivity, ContextBudgetLedger, ContextClass,
    FileMode, FilesystemIntent, NormalizedManagedPath, NormalizedResource, ResourceProvenance,
    declared_limit, tokens_for_bytes,
};
use commonkit_contracts::{Sha256Digest, StableId};

struct Fixture {
    store: ArtifactStore,
    _root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(root.path()).expect("store");
        Self { store, _root: root }
    }

    fn file(&self, path: &str, contents: &str) -> NormalizedResource {
        let content = self
            .store
            .put(contents.as_bytes(), ContentSensitivity::Portable)
            .expect("put");
        NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse(path).expect("path"),
                content,
                mode: Some(FileMode::parse(0o600).expect("mode")),
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: StableId::parse("apm").expect("provider"),
                provider_version: "0.25.0".into(),
                input_digest: Sha256Digest::parse(format!("sha256:{}", "a".repeat(64)))
                    .expect("digest"),
                source: path.into(),
            },
        }
    }

    fn measure(&self, resources: &[NormalizedResource], limit: Option<u64>) -> ContextBudgetLedger {
        ContextBudgetLedger::measure(resources, &self.store, limit).expect("measure")
    }
}

/// A skill with a fixed frontmatter block and a body of the requested size.
fn skill(body_bytes: usize) -> String {
    format!(
        "---\nname: example\ndescription: Does an example thing.\n---\n{}",
        "x".repeat(body_bytes)
    )
}

const FRONTMATTER_BYTES: u64 = "name: example\ndescription: Does an example thing.\n".len() as u64;

#[test]
fn router_cost_scales_with_skill_count_not_skill_size() {
    let fixture = Fixture::new();

    let small = fixture.measure(
        &[
            fixture.file("skills/one/SKILL.md", &skill(10)),
            fixture.file("skills/two/SKILL.md", &skill(10)),
        ],
        None,
    );
    let large = fixture.measure(
        &[
            fixture.file("skills/one/SKILL.md", &skill(50_000)),
            fixture.file("skills/two/SKILL.md", &skill(50_000)),
        ],
        None,
    );

    assert_eq!(small.skill_count, 2);
    assert_eq!(large.skill_count, 2);
    // A body 5000x larger costs the budget nothing extra.
    assert_eq!(small.router_tokens, large.router_tokens);
    assert_eq!(small.charged_tokens(), large.charged_tokens());
    // The body is still reported, so the growth remains visible.
    assert!(large.on_disk_tokens > small.on_disk_tokens);

    // Adding a third skill does raise the charged cost.
    let three = fixture.measure(
        &[
            fixture.file("skills/one/SKILL.md", &skill(10)),
            fixture.file("skills/two/SKILL.md", &skill(10)),
            fixture.file("skills/three/SKILL.md", &skill(10)),
        ],
        None,
    );
    assert_eq!(three.skill_count, 3);
    assert!(three.router_tokens > small.router_tokens);
}

#[test]
fn editing_always_on_text_leaves_router_cost_unchanged() {
    let fixture = Fixture::new();
    let skills = [
        fixture.file("skills/one/SKILL.md", &skill(64)),
        fixture.file("skills/two/SKILL.md", &skill(64)),
    ];

    let short = fixture.measure(
        &[
            vec![fixture.file("CLAUDE.md", "Be concise.")],
            skills.to_vec(),
        ]
        .concat(),
        None,
    );
    let long = fixture.measure(
        &[
            vec![fixture.file("CLAUDE.md", &"Be concise. ".repeat(500))],
            skills.to_vec(),
        ]
        .concat(),
        None,
    );

    assert!(long.always_on_tokens > short.always_on_tokens);
    assert_eq!(long.router_tokens, short.router_tokens);
    assert_eq!(long.on_disk_tokens, short.on_disk_tokens);
}

#[test]
fn agents_claude_and_hook_prose_are_always_on() {
    let fixture = Fixture::new();
    let ledger = fixture.measure(
        &[
            fixture.file("AGENTS.md", "agents"),
            fixture.file("CLAUDE.md", "claude"),
            fixture.file("hooks/pre-commit.md", "hook prose"),
            // Not agent context: measured by nobody, charged to nobody.
            fixture.file("scripts/build.sh", "#!/bin/sh\nexit 0\n"),
            fixture.file("hooks/pre-commit.sh", "#!/bin/sh\nexit 0\n"),
        ],
        None,
    );

    assert_eq!(ledger.skill_count, 0);
    assert_eq!(ledger.router_tokens, 0);
    assert_eq!(ledger.on_disk_tokens, 0);
    assert_eq!(ledger.entries.len(), 3);
    assert!(
        ledger
            .entries
            .iter()
            .all(|entry| entry.class == ContextClass::AlwaysOn)
    );
    assert!(ledger.always_on_tokens > 0);
}

#[test]
fn skill_frontmatter_is_router_text_and_body_is_on_disk() {
    let fixture = Fixture::new();
    let body = 400;
    let ledger = fixture.measure(&[fixture.file("skills/one/SKILL.md", &skill(body))], None);

    let router = ledger
        .entries
        .iter()
        .find(|entry| entry.class == ContextClass::Router)
        .expect("router entry");
    let on_disk = ledger
        .entries
        .iter()
        .find(|entry| entry.class == ContextClass::OnDisk)
        .expect("on-disk entry");

    assert_eq!(router.bytes, FRONTMATTER_BYTES);
    assert_eq!(on_disk.bytes, body as u64);
    assert!(ContextClass::Router.is_charged());
    assert!(!ContextClass::OnDisk.is_charged());
    // The on-disk body is excluded from the charged total.
    assert_eq!(ledger.charged_tokens(), ledger.router_tokens);
}

#[test]
fn skill_without_frontmatter_contributes_no_router_text() {
    let fixture = Fixture::new();
    let ledger = fixture.measure(
        &[
            fixture.file("skills/bare/SKILL.md", "# Bare skill\n\nNo frontmatter."),
            fixture.file("skills/unterminated/SKILL.md", "---\nname: broken\nstill open"),
        ],
        None,
    );

    assert_eq!(ledger.skill_count, 2);
    assert_eq!(ledger.router_tokens, 0);
    assert!(ledger.on_disk_tokens > 0);
}

#[test]
fn absent_limit_reports_figures_and_never_fails() {
    let fixture = Fixture::new();
    let ledger = fixture.measure(
        &[
            fixture.file("CLAUDE.md", &"prose ".repeat(10_000)),
            fixture.file("skills/one/SKILL.md", &skill(1_000)),
        ],
        None,
    );

    assert_eq!(ledger.limit, None);
    assert!(!ledger.over_limit());
    assert_eq!(ledger.remaining_tokens(), None);
    assert!(ledger.charged_tokens() > 0);
}

#[test]
fn declared_limit_reports_overage_without_blocking_measurement() {
    let fixture = Fixture::new();
    let resources = [fixture.file("CLAUDE.md", &"prose ".repeat(1_000))];

    let under = fixture.measure(&resources, Some(100_000));
    assert!(!under.over_limit());
    assert_eq!(
        under.remaining_tokens(),
        Some(100_000 - under.charged_tokens())
    );

    let over = fixture.measure(&resources, Some(10));
    assert!(over.over_limit());
    // Overage saturates rather than underflowing.
    assert_eq!(over.remaining_tokens(), Some(0));
    // Measurement still succeeded and reports the same figures.
    assert_eq!(over.charged_tokens(), under.charged_tokens());
}

#[test]
fn token_counting_is_deterministic_and_documented() {
    let fixture = Fixture::new();
    let resources = [
        fixture.file("CLAUDE.md", "some instructions"),
        fixture.file("skills/one/SKILL.md", &skill(123)),
    ];

    let first = fixture.measure(&resources, Some(1_000));
    let second = fixture.measure(&resources, Some(1_000));
    assert_eq!(first, second);

    // Rounds up, so any non-empty content costs at least one token.
    assert_eq!(tokens_for_bytes(0), 0);
    assert_eq!(tokens_for_bytes(1), 1);
    assert_eq!(tokens_for_bytes(BYTES_PER_TOKEN), 1);
    assert_eq!(tokens_for_bytes(BYTES_PER_TOKEN + 1), 2);
}

#[test]
fn loadout_without_context_budget_is_report_only() {
    let spec = serde_json::json!({ "files": {}, "hooks": [] });
    assert_eq!(declared_limit(&spec).expect("absent budget"), None);

    let explicit_null = serde_json::json!({ "contextBudget": null });
    assert_eq!(declared_limit(&explicit_null).expect("null budget"), None);
}

#[test]
fn declared_context_budget_is_read_from_the_loadout_spec() {
    let spec = serde_json::json!({ "contextBudget": { "maxTotalTokens": 12_000 } });
    assert_eq!(declared_limit(&spec).expect("budget"), Some(12_000));
}

#[test]
fn malformed_context_budget_is_rejected_rather_than_ignored() {
    // A typo must not silently disable a budget someone meant to enforce.
    for spec in [
        serde_json::json!({ "contextBudget": { "maxTokens": 12_000 } }),
        serde_json::json!({ "contextBudget": { "maxTotalTokens": 0 } }),
        serde_json::json!({ "contextBudget": { "maxTotalTokens": -5 } }),
        serde_json::json!({ "contextBudget": { "maxTotalTokens": "12000" } }),
        serde_json::json!({ "contextBudget": {} }),
    ] {
        assert!(
            declared_limit(&spec).is_err(),
            "expected rejection for {spec}"
        );
    }
}

#[test]
fn non_file_intents_are_ignored_rather_than_rejected() {
    let fixture = Fixture::new();
    let directory = NormalizedResource {
        intent: FilesystemIntent::Directory {
            path: NormalizedManagedPath::parse("skills").expect("path"),
            mode: None,
            exact: false,
        },
        provenance: ResourceProvenance {
            provider_id: StableId::parse("apm").expect("provider"),
            provider_version: "0.25.0".into(),
            input_digest: Sha256Digest::parse(format!("sha256:{}", "a".repeat(64)))
                .expect("digest"),
            source: "skills".into(),
        },
    };

    let ledger = fixture.measure(
        &[directory, fixture.file("skills/one/SKILL.md", &skill(16))],
        None,
    );

    assert_eq!(ledger.skill_count, 1);
    assert!(ledger.router_tokens > 0);
}
