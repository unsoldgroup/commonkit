import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("v1 completion claims remain tied to product-level evidence", async () => {
  const [audit, support, headless] = await Promise.all([
    read("docs/V1-COMPLETION-AUDIT.md"),
    read("docs/SUPPORT-MATRIX.md"),
    read("docs/HEADLESS-DOMAINS.md"),
  ]);
  for (let flow = 1; flow <= 12; flow += 1) {
    assert.match(audit, new RegExp(`\\| ${flow} \\|`), `missing flow ${flow}`);
  }
  assert.match(audit, /Do not mark CommonKit v1 release-ready/);
  assert.match(audit, /host-key confirmation.*not (?:performed|recorded)/i);
  assert.match(audit, /commit-bound historical evidence/i);
  assert.match(audit, /does not establish.*current HEAD/i);
  assert.match(audit, /Production Apple signing\/notarization/);
  assert.match(audit, /native-platform/i);
  assert.match(audit, /does not use GitHub Actions/i);
  assert.match(audit, /first eight runtime flows.*locally implemented/i);
  assert.doesNotMatch(support, /deliberately disabled/);
  assert.match(support, /have not been demonstrated with production notarization\/signing/);
  assert.match(support, /commit-bound.*e25785d/i);
  assert.match(support, /commit-bound.*e98dee5/i);
  assert.match(support, /real-SSH.*not (?:performed|recorded)/i);
  for (const scheme of ["env://", "file://", "bws://", "keychain://"]) {
    assert.match(headless, new RegExp(scheme.replaceAll("/", "\\/")));
  }
});
