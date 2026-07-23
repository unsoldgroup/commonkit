import assert from "node:assert/strict";
import { access, readFile, readdir } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("repository instructions prohibit GitHub Actions", async () => {
  const [claude, agents] = await Promise.all([
    read("CLAUDE.md"),
    read("AGENTS.md"),
  ]);

  for (const instructions of [claude, agents]) {
    assert.match(instructions, /does not use GitHub Actions/i);
    assert.match(instructions, /local and manually invoked/i);
    assert.match(instructions, /do not add.*\.github\/workflows/is);
    assert.match(instructions, /must not.*completion.*gate/is);
  }
});

test("no GitHub Actions workflows are committed", async () => {
  const workflowsUrl = new URL("../.github/workflows/", import.meta.url);
  const entries = await readdir(workflowsUrl).catch((error) => {
    if (error.code === "ENOENT") return [];
    throw error;
  });
  assert.deepEqual(entries, []);
});

test("manual validation and release entrypoints remain committed", async () => {
  for (const path of [
    "scripts/installed-lifecycle.sh",
    "scripts/build-release-cli.sh",
    "scripts/prepare-release-config.mjs",
    "scripts/verify-release-assets.mjs",
    "docs/RELEASING.md",
    "docs/SUPPORT-MATRIX.md",
  ]) {
    await access(new URL(`../${path}`, import.meta.url));
  }

  const release = await read("docs/RELEASING.md");
  assert.match(release, /trusted release workstation/i);
  assert.match(release, /manually invoked platform runner/i);
  assert.match(release, /does not use GitHub Actions/i);
});
