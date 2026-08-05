/* Design harness: renders the real view functions against realistic fixtures so the
   panel can be screenshotted without a running daemon. Not shipped. */
import "../src/style.css";
import { onboardingPanel, defaultOnboardingDraft, type OnboardingViewState } from "../src/onboarding-view.ts";
import { statusPanel } from "../src/status-panel.ts";
import { managementPanel } from "../src/management-view.ts";
import { updatePanel } from "../src/updater-view.ts";
import { icon } from "../src/icons.ts";
import { driveInstrument } from "../src/instruments.ts";
import { readPanel } from "../src/readings.ts";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "../src/contracts.ts";

const params = new URLSearchParams(location.search);
const screen = params.get("screen") ?? "onboarding";
const step = Number(params.get("step") ?? "1") as 1 | 2 | 3 | 4;

const snapshot: DesktopSnapshot = {
  status: {
    apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1, runtimeVersion: "0.1.0",
    state: "drifted", activeTarget: "al-macbook", activeLoadout: "personal",
    lastDriftCheckUnixMs: Date.parse("2026-08-05T21:40:00Z"), lastDriftErrorCode: null,
  },
  capabilities: [], lastEventId: 42, events: [],
  gitSync: { state: "clean", branch: "main", revision: "8d7b180" },
  policy: { state: "ready", violations: [] },
};

const management: ManagementSnapshot = {
  aboutMe: { error: "about_me_domain_unconfigured" },
  plans: {
    plan: {
      id: "sha256:9f2c4a17be08d3", targetId: "al-macbook", risk: "medium",
      operations: [
        { summary: "Write agent instructions", path: "~/CommonKitManaged/CLAUDE.md", risk: "low", provenance: { layer: "personal" } },
        { summary: "Install skill bundle", path: "~/CommonKitManaged/.claude/skills/ast-grep", risk: "low", provenance: { layer: "personal" } },
        { summary: "Replace MCP relay declaration", path: "~/CommonKitManaged/.mcp.json", risk: "medium", provenance: { layer: "project:commonkit" } },
      ],
    },
  },
  credentials: { credentials: [] },
  snapshots: { error: "snapshot_domain_unconfigured" },
  relay: { state: "running", servers: [
    { id: "context-mode", name: "context-mode", state: "ready", transport: "stdio" },
    { id: "linear", name: "linear", state: "ready", transport: "https" },
    { id: "posthog-local", name: "posthog-local", state: "degraded", transport: "http" },
  ] },
  schedule: { enabled: true, intervalSeconds: 900 },
  diagnostics: {},
};

const targets: TargetInventorySnapshot = {
  selected: ["al-macbook"],
  targets: [
    { id: "al-macbook", identityDigest: "sha256:1f3a…", transport: { type: "local" } },
    { id: "hostinger-vps", identityDigest: "sha256:c07b…", transport: { type: "ssh", host: "srv1833518", user: "root", port: 22 } },
  ],
};

const settings = {
  autostart: true,
  configDirectory: "/Users/al/Library/Application Support/CommonKit",
  stateDirectory: "/Users/al/Library/Application Support/CommonKit/state",
  repository: "al-unsoldgroup/commonkit",
  targetRoot: "/Users/al/CommonKitManaged",
};

const onboarding: OnboardingViewState = {
  step,
  auth: step === 1 && params.get("auth") === "out" ? { state: "signedOut" } : { state: "authenticated", login: "al-unsoldgroup" },
  draft: { ...defaultOnboardingDraft(), kitDirectory: "/Users/al/.config/commonkit", targetRoot: "/Users/al/CommonKitManaged", computerName: "al-macbook" },
  message: step === 4 ? "" : "",
  submitting: false,
};

const nav = [
  ["Overview", [["status", "Status"]]],
  ["Context", [["profile", "My work profile"]]],
  ["Operate", [["plans", "Changes"], ["credentials", "Credentials"], ["snapshots", "Data"], ["aboutMe", "About Me"], ["relay", "MCP connections"], ["schedule", "Drift checks"]]],
  ["System", [["diagnostics", "Diagnostics"], ["settings", "Settings"]]],
] as const;

const active = screen === "onboarding" ? "onboarding" : screen;
const railGroups = screen === "onboarding"
  ? `<section class="rail-group"><p>Setup</p><a href="#" aria-current="page">Get started</a></section>`
  : nav.map(([group, items]) => `<section class="rail-group"><p>${group}</p>${items.map(([id, label]) => `<a href="#${id}"${id === active ? ' aria-current="page"' : ""}>${label}</a>`).join("")}</section>`).join("");

const body = screen === "onboarding" ? onboardingPanel(onboarding)
  : screen === "status" ? statusPanel(snapshot, targets, management, settings)
  : screen === "settings" ? updatePanel({ kind: "idle" }, settings)
  : managementPanel(screen as "plans", management as unknown as Record<string, unknown>);

document.querySelector("#app")!.innerHTML =
  `<aside class="rail"><div class="mark">${icon.mark()}<b>CommonKit</b></div><nav>${railGroups}</nav><p class="rail-foot">v0.1.0</p></aside><main class="deck">${body}</main>`;

const root = document.querySelector("#app")!;
for (const [spec, reading] of readPanel(screen === "onboarding" ? null : snapshot, screen === "onboarding" ? null : management)) {
  driveInstrument(root, spec, reading);
}

