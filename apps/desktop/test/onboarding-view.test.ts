import assert from "node:assert/strict";
import test from "node:test";
import { defaultOnboardingDraft, onboardingPanel, type OnboardingViewState } from "../src/onboarding-view.ts";

function state(overrides: Partial<OnboardingViewState> = {}): OnboardingViewState {
  return {
    step: 1,
    auth: { state: "signedOut" },
    draft: defaultOnboardingDraft(),
    message: "",
    submitting: false,
    ...overrides,
  };
}

test("signed-out onboarding starts with one understandable GitHub action", () => {
  const html = onboardingPanel(state());
  assert.match(html, /Connect GitHub/);
  assert.match(html, /private repository/i);
  assert.doesNotMatch(html, /Kit directory|Loadout|Managed target root|Materialize|Provider/);
});

test("wizard shows four steps and marks only the current step", () => {
  const html = onboardingPanel(state({ step: 2, auth: { state: "authenticated", login: "developer" } }));
  assert.equal((html.match(/<li[\s>]/g) ?? []).length, 4);
  assert.equal((html.match(/aria-current="step"/g) ?? []).length, 1);
  assert.match(html, /This computer/);
  assert.match(html, /Project layer/);
  assert.match(html, /Station layer/);
  assert.match(html, /name="projectLoadout"/);
  assert.match(html, /name="targetOverride"/);
});

test("authenticated setup uses plain language and hides implementation details", () => {
  const html = onboardingPanel(state({ auth: { state: "authenticated", login: "developer" } }));
  assert.match(html, /developer/);
  assert.match(html, /New kit/);
  assert.doesNotMatch(html, /Kit directory|Loadout|Managed target root|Materialize|Provider/);
});

test("import step keeps provider details collapsed and pinned", () => {
  const apm = defaultOnboardingDraft();
  apm.provider = "apm";
  const html = onboardingPanel(state({ step: 3, auth: { state: "authenticated", login: "developer" }, draft: apm }));
  assert.match(html, /Existing settings/);
  assert.match(html, /<details/);
  assert.match(html, /value="0.25.0"/);
  assert.match(html, /apmLockfile/);
  assert.doesNotMatch(html, /type="password"/);
});

test("review explains safety and escapes every user-controlled value", () => {
  const draft = defaultOnboardingDraft();
  draft.repositoryName = `<script>alert(1)</script>`;
  draft.computerName = `mac"><img src=x>`;
  const html = onboardingPanel(state({ step: 4, auth: { state: "authenticated", login: "developer" }, draft }));
  assert.match(html, /Nothing is applied by this step/);
  assert.match(html, /saves the setup locally/i);
  assert.match(html, /Prepare plan/);
  assert.doesNotMatch(html, /<script>|<img/);
});

test("onboarding messages are announced and escaped", () => {
  const html = onboardingPanel(state({ message: "<script>alert(1)</script>" }));
  assert.match(html, /aria-live="polite"/);
  assert.doesNotMatch(html, /<script>/);
});
