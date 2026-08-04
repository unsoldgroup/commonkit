import assert from "node:assert/strict";
import test from "node:test";
import {
  advanceProfileQuestion, cancelProfileDraft, coreProfileQuestions, initialProfileDraft,
  recoveryRecipients, reviewProfileAnswer,
} from "../src/profile-interview-controller.ts";

test("the controller exposes the complete ordered core catalog one question at a time", () => {
  const state = initialProfileDraft();
  assert.equal(coreProfileQuestions.length, 38);
  assert.equal(state.question?.fieldId, "identity.display_name");
  reviewProfileAnswer(state, "Alex");
  assert.equal(state.answers.size, 0);
  advanceProfileQuestion(state, true);
  assert.equal(state.answers.get("identity.display_name"), "Alex");
  assert.equal(state.question?.fieldId, "identity.pronouns");
});

test("skip and cancel persist neither unconfirmed values nor an interview transcript", () => {
  const state = initialProfileDraft();
  reviewProfileAnswer(state, "unconfirmed private text");
  advanceProfileQuestion(state, false);
  assert.equal(state.answers.size, 0);
  state.answers.set("identity.role", "Engineer");
  state.recipientsText = "age1public";
  cancelProfileDraft(state);
  assert.equal(state.answers.size, 0);
  assert.equal(state.pendingValue, "");
  assert.equal(state.recipientsText, "");
});

test("recovery recipients are distinct public lines", () => {
  assert.deepEqual(recoveryRecipients("age1a\nage1b\nage1a\n\nage1c"), ["age1a", "age1b", "age1c"]);
});
