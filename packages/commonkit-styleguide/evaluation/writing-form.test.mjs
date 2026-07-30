import assert from "node:assert/strict";
import test from "node:test";

import { scoreWritingForm } from "./writing-form.mjs";

test("matches upstream categories and short-output normalization", () => {
  const result = scoreWritingForm(
    "It's important to note that the file is written by the tool; utilize it.",
  );

  assert.equal(result.words, 14);
  assert.equal(result.violations.contraction, 1);
  assert.equal(result.violations.passive_voice, 1);
  assert.equal(result.violations.semicolon, 1);
  assert.equal(result.violations.banned_word, 1);
  assert.equal(result.totalPer100Words, 28.57);
});

test("excludes fenced and inline code from writing-form violations", () => {
  const result = scoreWritingForm(
    "Run this command:\n\n```js\nconst robust = `it's set`;\n```\n\nUse `ensure;` as the key.",
  );

  assert.equal(result.total, 0);
  assert.equal(result.words, 7);
});

test("detects passive voice, gerunds, long sentences, and terminology markers", () => {
  const result = scoreWritingForm(
    "The file was written. The worker is processing data. " +
      "This robust component utilizes a comprehensive configuration of many values and provides a delightful interface for every operator in the organization today.",
  );

  assert.equal(result.violations.passive_voice, 1);
  assert.equal(result.violations.ing_main_verb, 1);
  assert.equal(result.violations["long_sentence(>20w)"], 1);
  assert.equal(result.violations.marketing_adjective, 2);
  assert.equal(result.violations.banned_word, 2);
  assert.equal(result.violations.nominalization, 2);
});

test("keeps em dashes outside the violation total for upstream parity", () => {
  const result = scoreWritingForm("The parser reads the file — then it exits.");

  assert.equal(result.emDashes, 1);
  assert.equal(result.total, 0);
});
