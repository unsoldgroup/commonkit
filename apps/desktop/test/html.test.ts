import assert from "node:assert/strict";
import test from "node:test";

import { escapeHtml } from "../src/html.ts";

test("HTML escaping handles every sensitive character and non-string values", () => {
  assert.equal(escapeHtml(`<>&"'`), "&lt;&gt;&amp;&quot;&#39;");
  assert.equal(escapeHtml(42), "42");
  assert.equal(escapeHtml(null), "");
});
