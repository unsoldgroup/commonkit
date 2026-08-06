import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const installerUrl = new URL("../apps/session-board/deploy/install-mac.sh", import.meta.url);
const templateUrl = new URL(
  "../apps/session-board/deploy/templates/cloud.unsold.session-board-reporter.plist",
  import.meta.url,
);

test("the mac reporter runs from a managed bundle instead of a Git checkout", async () => {
  const [installer, template] = await Promise.all([
    readFile(installerUrl, "utf8"),
    readFile(templateUrl, "utf8"),
  ]);

  assert.match(installer, /bun build/);
  assert.match(installer, /session-board-reporter\.js/);
  assert.match(installer, /@@RUNTIME_DIR@@/);
  assert.doesNotMatch(template, /@@REPO_DIR@@/);
  assert.doesNotMatch(template, /<key>WorkingDirectory<\/key>/);
  assert.match(template, /@@RUNTIME_DIR@@\/session-board-reporter\.js/);
  assert.match(installer, /for attempt in 1 2 3/);
  assert.match(installer, /launchctl bootstrap/);
});
