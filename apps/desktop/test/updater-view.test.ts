import assert from "node:assert/strict";
import test from "node:test";

import { updatePanel, type UpdateUiState } from "../src/updater-view.ts";

test("available update shows escaped notes and requires a distinct install action", () => {
  const state: UpdateUiState = {
    kind: "available",
    update: {
      currentVersion: "0.1.0",
      version: "0.2.0",
      notes: "Security fixes <script>alert(1)</script>",
      publishedAt: "2026-07-18T00:00:00Z",
    },
  };
  const html = updatePanel(state);
  assert.match(html, /Version 0\.2\.0/);
  assert.match(html, /Security fixes/);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /id="install-update"/);
  assert.match(html, /Install update/);
  assert.doesNotMatch(html, /checked/);
});

test("checking never implies consent and install failures remain visible", () => {
  assert.match(updatePanel({ kind: "idle" }), /Check for updates/);
  assert.doesNotMatch(updatePanel({ kind: "checking" }), /install-update/);
  assert.match(updatePanel({ kind: "error", message: "Signature verification failed" }), /Signature verification failed/);
});

test("settings explains this computer and exposes launch-at-login controls", () => {
  const html = updatePanel(
    { kind: "idle" },
    {
      autostart: true,
      configDirectory: "/Users/developer/Library/Application Support/CommonKit",
      stateDirectory: "/Users/developer/Library/Application Support/CommonKit/state",
      repository: "https://github.com/al/commonkit.git",
      targetRoot: "/Users/developer/CommonKitManaged",
    },
  );

  assert.match(html, /This computer/i);
  assert.match(html, /github\.com\/al\/commonkit/);
  assert.match(html, /Launch at login/i);
  assert.match(html, /id="autostart-disable"/);
  assert.match(html, /Reconnect or reset/i);
});
