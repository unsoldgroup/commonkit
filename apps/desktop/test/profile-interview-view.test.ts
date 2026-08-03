import assert from "node:assert/strict";
import test from "node:test";
import { profileInterviewPanel, type ProfileInterviewState } from "../src/profile-interview-view.ts";

function state(overrides: Partial<ProfileInterviewState> = {}): ProfileInterviewState {
  return {
    question: {
      fieldId: "communication.tone",
      label: "What tone should agents use?",
      help: "Describe the working tone you prefer.",
      source: "CommonKit",
      valueType: "string",
    },
    position: 6,
    total: 38,
    pendingValue: "",
    awaitingConfirmation: false,
    submitting: false,
    message: "",
    ...overrides,
  };
}

test("interview presents one optional question with its schema source", () => {
  const html = profileInterviewPanel(state());
  assert.match(html, /6 of 38/);
  assert.match(html, /What tone should agents use/);
  assert.match(html, /CommonKit core profile/);
  assert.match(html, />Skip</);
  assert.equal((html.match(/name="answer"/g) ?? []).length, 1);
  assert.doesNotMatch(html, /transcript/i);
});

test("confirmation renders the exact escaped local value before encryption", () => {
  const html = profileInterviewPanel(state({
    pendingValue: `<script>private</script>`,
    awaitingConfirmation: true,
  }));
  assert.match(html, /Confirm this exact value/);
  assert.match(html, /&lt;script&gt;private&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /Confirm and continue/);
});

test("organization extensions are visibly attributed", () => {
  const question = state().question!;
  const html = profileInterviewPanel(state({
    question: { ...question, source: "Acme profile v2" },
  }));
  assert.match(html, /Extension from Acme profile v2/);
});

test("completed interview requires three public recovery paths before local encryption", () => {
  const html = profileInterviewPanel(state({ question: null, recipientsText: "" }));
  assert.match(html, /this device/i);
  assert.match(html, /offline recovery key/i);
  assert.match(html, /password manager/i);
  assert.match(html, /public age1/i);
  assert.doesNotMatch(html, /private identity.*value/i);
});
