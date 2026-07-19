import assert from "node:assert/strict";
import test from "node:test";
import { onboardingPanel } from "../src/onboarding-view.ts";

test("guided onboarding exposes only pinned provider-specific inputs", () => {
  assert.match(onboardingPanel("native"), /Materialize first plan/);
  assert.match(onboardingPanel("native"), /publishRegistration/);
  assert.doesNotMatch(onboardingPanel("native"), /providerVersion/);
  assert.match(onboardingPanel("apm"), /value="0.25.0"/);
  assert.match(onboardingPanel("apm"), /apmLockfile/);
  assert.match(onboardingPanel("chezmoi"), /value="2.70.4"/);
  assert.match(onboardingPanel("chezmoi"), /chezmoiSource/);
  assert.doesNotMatch(onboardingPanel("apm"), /type="password"/);
});

test("onboarding status is escaped", () => {
  assert.doesNotMatch(onboardingPanel("native", "<script>alert(1)</script>"), /<script>/);
});
