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
    apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1, runtimeVersion: "0.2.0",
    state: "healthy", activeTarget: "local-workstation", activeLoadout: "personal",
    lastDriftCheckUnixMs: null, lastDriftErrorCode: null,
  },
  panel: {
    contractVersion: "commonkit.panel/v1",
    observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z"),
    channels: {
      skills: { state: "available", value: 59, source: "skillInventory", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z") },
      mcpServers: { state: "empty", value: 0, source: "relayStatus", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z") },
      changes: { state: "empty", value: 0, source: "planStore", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z") },
      credentials: { state: "empty", value: 0, source: "credentialReferences", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z") },
      devices: { state: "available", value: 1, source: "targetInventory", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z") },
      agentSessions: { state: "available", value: 11, source: "sessionBoardReporter", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z") },
      drift: { state: "unchecked", source: "driftScheduler", observedAtUnixMs: Date.parse("2026-08-06T20:00:00Z"), reason: "never_checked" },
    },
  },
  capabilities: [], lastEventId: 42, events: [],
  gitSync: { state: "clean", branch: "main", revision: "2f6a911" },
  policy: { state: "ready", violations: [] },
};

const management: ManagementSnapshot = {
  aboutMe: { error: "about_me_domain_unconfigured" },
  plans: {},
  credentials: { credentials: [] },
  snapshots: { snapshots: [] },
  relay: { state: "running", servers: [] },
  schedule: { enabled: true, intervalSeconds: 900 },
  diagnostics: {},
};

const targets: TargetInventorySnapshot = {
  selected: ["local-workstation"],
  targets: [
    { id: "local-workstation", identityDigest: "sha256:1f3a…", transport: { type: "local" } },
  ],
};

const settings = {
  autostart: true,
  configDirectory: "/Users/demo/Library/Application Support/CommonKit",
  stateDirectory: "/Users/demo/Library/Application Support/CommonKit/state",
  repository: "example-org/my-commonkit",
  targetRoot: "/Users/demo/CommonKitManaged",
};

const onboarding: OnboardingViewState = {
  step,
  auth: step === 1 && params.get("auth") === "out" ? { state: "signedOut" } : { state: "authenticated", login: "octocat" },
  draft: { ...defaultOnboardingDraft(), kitDirectory: "/Users/demo/.config/commonkit", targetRoot: "/Users/demo/CommonKitManaged", computerName: "local-workstation" },
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
  `<aside class="rail"><div class="mark">${icon.mark()}<b>CommonKit</b></div><nav>${railGroups}</nav><p class="rail-foot">v0.2.0</p></aside><main class="deck">${body}</main>`;

const root = document.querySelector("#app")!;
for (const [spec, reading] of readPanel(
  screen === "onboarding" ? null : snapshot,
  screen === "onboarding" ? null : management,
  screen === "onboarding" ? null : targets,
)) {
  driveInstrument(root, spec, reading);
}
