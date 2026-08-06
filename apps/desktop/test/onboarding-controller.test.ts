import assert from "node:assert/strict";
import test from "node:test";
import { defaultOnboardingDraft } from "../src/onboarding-view.ts";
import {
  applyOnboardingValues,
  onboardingRequest,
  stableComputerId,
} from "../src/onboarding-controller.ts";

test("wizard values survive navigation and provider rerenders", () => {
  const draft = defaultOnboardingDraft();
  applyOnboardingValues(draft, new Map(Object.entries({
    repositoryName: "demo-kit",
    mode: "create",
  })));
  applyOnboardingValues(draft, new Map(Object.entries({
    computerName: "local-workstation",
    targetRoot: "/tmp/commonkit-test",
  })));
  assert.equal(draft.repositoryName, "demo-kit");
  assert.equal(draft.computerName, "local-workstation");
  assert.equal(draft.targetRoot, "/tmp/commonkit-test");
});

test("final guided request derives the authenticated repository owner", () => {
  const draft = defaultOnboardingDraft();
  Object.assign(draft, {
    repositoryName: "demo-kit",
    kitDirectory: "/Users/developer/.config/commonkit/demo-kit",
    computerName: "local-workstation",
    targetRoot: "/Users/developer/CommonKitManaged",
  });
  assert.deepEqual(onboardingRequest(draft, "developer"), {
    mode: "create",
    repository: "developer/demo-kit",
    kitDirectory: "/Users/developer/.config/commonkit/demo-kit",
    loadout: "personal",
    target: "local-workstation",
    targetRoot: "/Users/developer/CommonKitManaged",
    provider: "native",
    publishRegistration: true,
  });
});

test("human computer names become valid stable target identifiers", () => {
  assert.equal(stableComputerId("Local-Workstation"), "local-workstation");
  assert.equal(stableComputerId("  Élodies MacBook Pro  "), "elodies-macbook-pro");
  assert.equal(stableComputerId("2026 workstation"), "machine-2026-workstation");
  assert.equal(stableComputerId("___"), "workstation");

  const draft = defaultOnboardingDraft();
  draft.computerName = "Local-Workstation";
  assert.equal(onboardingRequest(draft, "developer").target, "local-workstation");
});

test("provider pins are added only for the selected import", () => {
  const draft = defaultOnboardingDraft();
  Object.assign(draft, {
    provider: "apm",
    providerExecutable: "/opt/apm",
    apmManifest: "apm.yml",
    apmLockfile: "apm.lock.yaml",
    apmPolicy: "apm-policy.yml",
  });
  assert.equal(onboardingRequest(draft, "developer").providerVersion, "0.25.0");
  assert.equal(onboardingRequest(draft, "developer").apmLockfile, "apm.lock.yaml");
  assert.equal("chezmoiConfig" in onboardingRequest(draft, "developer"), false);
});

test("advanced composition selectors are portable and omitted when blank", () => {
  const draft = defaultOnboardingDraft();
  assert.equal("projectLoadout" in onboardingRequest(draft, "developer"), false);
  assert.equal("targetOverride" in onboardingRequest(draft, "developer"), false);

  applyOnboardingValues(draft, new Map(Object.entries({
    projectLoadout: "project-web",
    targetOverride: "target-macbook",
  })));
  const request = onboardingRequest(draft, "developer");
  assert.equal(request.projectLoadout, "project-web");
  assert.equal(request.targetOverride, "target-macbook");
});
