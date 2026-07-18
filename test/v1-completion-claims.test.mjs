import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("v1 completion claims remain tied to product-level evidence", async () => {
  const [audit, support] = await Promise.all([
    read("docs/V1-COMPLETION-AUDIT.md"),
    read("docs/SUPPORT-MATRIX.md"),
  ]);
  for (let flow = 1; flow <= 12; flow += 1) {
    assert.match(audit, new RegExp(`\\| ${flow} \\|`), `missing flow ${flow}`);
  }
  assert.match(audit, /Do not mark CommonKit v1 complete or release-ready/);
  assert.match(audit, /production daemon does not construct\/run a `DriftScheduler`/);
  assert.match(audit, /management panels are placeholders/i);
  assert.doesNotMatch(support, /deliberately disabled/);
  assert.match(support, /have not been demonstrated with production notarization\/signing/);
});
