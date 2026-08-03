import assert from "node:assert/strict";
import test from "node:test";

import { managementPanel } from "../src/management-view.ts";

test("management panels render live domain state without secret values", () => {
  const html = managementPanel("credentials", {
    credentials: { credentials: [{ reference: "bws://shared/GITHUB_TOKEN", readiness: "ready" }] },
  });

  assert.match(html, /bws:\/\/shared\/GITHUB_TOKEN/);
  assert.match(html, /ready/);
  assert.doesNotMatch(html, /secretValue/);
});

test("about me panel explains encryption and keeps project memory separate", () => {
  const html = managementPanel("aboutMe", {
    aboutMe: {
      revision: 3,
      summary: "Call me Al.",
      pendingSuggestions: 1,
      suggestions: [{
        id: "suggestion-1",
        claim: { text: "Use fewer acronyms." },
        evidenceQuote: "Please use fewer acronyms.",
      }],
    },
  });
  assert.match(html, /Encrypted profile/i);
  assert.match(html, /Call me Al/);
  assert.match(html, /1 suggestion/i);
  assert.match(html, /Use fewer acronyms/);
  assert.match(html, /Please use fewer acronyms/);
  assert.match(html, /data-about-me-decision="accept"/);
  assert.match(html, /data-about-me-decision="reject"/);
  assert.match(html, /current loadout and project/i);
});

test("unconfigured about me panel contains a plain-language setup interview", () => {
  const html = managementPanel("aboutMe", {
    aboutMe: { error: "about_me_domain_unconfigured" },
  });

  assert.match(html, /What should agents call you/i);
  assert.match(html, /How should agents explain/i);
  assert.match(html, /What should agents never assume/i);
  assert.match(html, /id="about-me-setup"/);
  assert.match(html, /Nothing is saved until/i);
  assert.doesNotMatch(html, /about_me_domain_unconfigured/);
});

test("every operator screen states its objective before exposing controls", () => {
  const fixtures = {
    plans: { specDigest: "sha256:spec" },
    credentials: { credentials: [] },
    snapshots: { error: "snapshot_domain_unconfigured" },
    relay: { servers: [] },
    schedule: { enabled: false, intervalSeconds: 900 },
    diagnostics: { checks: [] },
  } as const;

  for (const [route, value] of Object.entries(fixtures)) {
    const html = managementPanel(route as keyof typeof fixtures, { [route]: value });
    assert.match(html, /class="screen-objective"/);
  }
});

test("credentials empty state offers an inline reference check without technical prompts", () => {
  const html = managementPanel("credentials", {
    credentials: { credentials: [] },
  });

  assert.match(html, /Setup needed/i);
  assert.match(html, /Credential reference/i);
  assert.match(html, /name="credential-reference"/);
  assert.match(html, /id="credential-readiness"/);
  assert.doesNotMatch(html, /id="credential-apply"/);
  assert.doesNotMatch(html, /destination ID/i);
});

test("an unconfigured data domain explains the prerequisite instead of exposing an error", () => {
  const html = managementPanel("snapshots", {
    snapshots: { error: "snapshot_domain_unconfigured" },
  });

  assert.match(html, /Setup needed/i);
  assert.match(html, /protect a database/i);
  assert.match(html, /Configure database/i);
  assert.doesNotMatch(html, /snapshot_domain_unconfigured/);
  assert.doesNotMatch(html, /id="snapshot-create"/);
});

test("an empty relay explains portable MCP connections and hides mutation controls", () => {
  const html = managementPanel("relay", {
    relay: { configured: false, servers: [] },
  });

  assert.match(html, /Setup needed/i);
  assert.match(html, /MCP connection/i);
  assert.match(html, /Add connection/i);
  assert.doesNotMatch(html, /id="relay-reconcile"/);
  assert.doesNotMatch(html, /id="relay-restart"/);
});

test("drift checks use an inline human-readable schedule instead of a numeric prompt", () => {
  const html = managementPanel("schedule", {
    schedule: { enabled: false, intervalSeconds: 900 },
  });

  assert.match(html, /Every 15 minutes/i);
  assert.match(html, /name="schedule-interval"/);
  assert.match(html, /value="900"/);
  assert.match(html, /id="schedule-enable"/);
  assert.match(html, /Read-only/i);
});

test("operator panels expose explicit fixed actions without rendering secret inputs", () => {
  assert.match(managementPanel("snapshots", { snapshots: { snapshots: [] } }), /id="snapshot-create"/);
  const relay = managementPanel("relay", { relay: { state: "healthy", servers: [{ id: "docs" }] } });
  assert.match(relay, /id="relay-restart"/);
  assert.doesNotMatch(relay, /id="relay-reconcile"/);
  assert.match(managementPanel("schedule", { schedule: { enabled: false } }), /id="schedule-enable"/);
  assert.match(managementPanel("credentials", { credentials: { credentials: [{ id: "api", readiness: "ready" }] } }), /id="credential-apply"/);
  assert.match(managementPanel("diagnostics", { diagnostics: { status: "healthy" } }), /id="diagnostics-export"/);
  assert.doesNotMatch(managementPanel("credentials", { credentials: {} }), /type="password"/);
});

test("plan review is human-readable and selects a bound digest without raw JSON", () => {
  const html = managementPanel("plans", { plans: {
    plan: {
      id: `sha256:${"a".repeat(64)}`,
      risk: "high",
      operations: [{ kind: "write_file", path: ".config/tool.json", provenance: { layer: "team" } }],
    },
    operation: { state: "running", completedOperations: 1, totalOperations: 3 },
  } });
  assert.match(html, /High risk/i);
  assert.match(html, /write file/i);
  assert.match(html, /team/i);
  assert.match(html, /1 of 3/i);
  assert.match(html, /name="plan-id"/);
  assert.doesNotMatch(html, /<pre>/);
});

test("changes distinguishes composition readiness from a generated zero-operation plan", () => {
  const composed = managementPanel("plans", { plans: { specDigest: "sha256:spec", trace: [] } });
  assert.match(composed, /Ready to inspect/i);
  assert.match(composed, /Generate plan/i);
  assert.doesNotMatch(composed, /Apply reviewed plan/);

  const current = managementPanel("plans", { plans: {
    id: `sha256:${"a".repeat(64)}`, targetId: "macbook", operations: [],
  } });
  assert.match(current, /Up to date/i);
  assert.doesNotMatch(current, /Apply reviewed plan/);
});

test("plan review renders the production Rust plan contract faithfully", () => {
  const digest = `sha256:${"a".repeat(64)}`;
  const html = managementPanel("plans", { plans: {
    schemaVersion: 1,
    contractVersion: "1.0.0",
    id: digest,
    targetId: "workstation",
    operations: [{
      id: digest,
      adapterId: "filesystem",
      kind: "update",
      resource: {
        resourceType: "file",
        resourceId: "editor-config",
        managedPath: ".config/editor.json",
      },
      risk: "high",
      requiresConfirmation: true,
      dependsOn: [],
      payloadDigest: digest,
      summary: "Update editor configuration",
    }],
  } });

  assert.match(html, /High risk/i);
  assert.match(html, /Update editor configuration/);
  assert.match(html, /\.config\/editor\.json/);
  assert.match(html, /filesystem/);
  assert.doesNotMatch(html, /\[object Object\]/);
  assert.doesNotMatch(html, /Unknown risk/i);
});

test("snapshot, relay, and diagnostics panels expose typed inventory controls", () => {
  const snapshots = managementPanel("snapshots", { snapshots: {
    authoritativeWriter: { databaseId: "catalog", targetId: "macbook" },
    snapshots: [{ id: "snap-1", databaseId: "catalog", createdAt: "2026-07-19T10:00:00Z" }],
  } });
  assert.match(snapshots, /Authoritative writer/i);
  assert.match(snapshots, /value="snap-1"/);
  assert.doesNotMatch(snapshots, /<pre>/);

  const relay = managementPanel("relay", { relay: {
    state: "healthy", upstreams: [{ id: "docs", state: "ready", transport: "stdio" }],
  } });
  assert.match(relay, /docs/);
  assert.match(relay, /ready/);

  const diagnostics = managementPanel("diagnostics", { diagnostics: {
    adapters: [{ id: "filesystem", state: "healthy", detail: "3 managed paths" }],
  } });
  assert.match(diagnostics, /filesystem/);
  assert.match(diagnostics, /3 managed paths/);
  assert.match(diagnostics, /id="diagnostics-refresh"/);
  assert.match(diagnostics, /id="diagnostics-export"/);
});
