import assert from "node:assert/strict";
import test from "node:test";
import { defaultOnboardingDraft } from "../src/onboarding-view.ts";
import { applyOnboardingValues, onboardingRequest } from "../src/onboarding-controller.ts";

test("wizard values survive navigation and provider rerenders", () => {
  const draft = defaultOnboardingDraft();
  applyOnboardingValues(draft, new Map(Object.entries({
    repositoryName: "al-kit",
    mode: "create",
  })));
  applyOnboardingValues(draft, new Map(Object.entries({
    computerName: "al-macbook",
    targetRoot: "/tmp/commonkit-test",
  })));
  assert.equal(draft.repositoryName, "al-kit");
  assert.equal(draft.computerName, "al-macbook");
  assert.equal(draft.targetRoot, "/tmp/commonkit-test");
});

test("final guided request derives the authenticated repository owner", () => {
  const draft = defaultOnboardingDraft();
  Object.assign(draft, {
    repositoryName: "al-kit",
    kitDirectory: "/Users/al/.config/commonkit/al-kit",
    computerName: "al-macbook",
    targetRoot: "/Users/al/CommonKitManaged",
  });
  assert.deepEqual(onboardingRequest(draft, "astemarie"), {
    mode: "create",
    repository: "astemarie/al-kit",
    kitDirectory: "/Users/al/.config/commonkit/al-kit",
    loadout: "personal",
    target: "al-macbook",
    targetRoot: "/Users/al/CommonKitManaged",
    provider: "native",
    publishRegistration: true,
  });
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
  assert.equal(onboardingRequest(draft, "astemarie").providerVersion, "0.25.0");
  assert.equal(onboardingRequest(draft, "astemarie").apmLockfile, "apm.lock.yaml");
  assert.equal("chezmoiConfig" in onboardingRequest(draft, "astemarie"), false);
});

test("advanced composition selectors are portable and omitted when blank", () => {
  const draft = defaultOnboardingDraft();
  assert.equal("projectLoadout" in onboardingRequest(draft, "astemarie"), false);
  assert.equal("targetOverride" in onboardingRequest(draft, "astemarie"), false);

  applyOnboardingValues(draft, new Map(Object.entries({
    projectLoadout: "project-web",
    targetOverride: "target-macbook",
  })));
  const request = onboardingRequest(draft, "astemarie");
  assert.equal(request.projectLoadout, "project-web");
  assert.equal(request.targetOverride, "target-macbook");
});
