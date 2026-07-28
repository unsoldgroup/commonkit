import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("onboarding documents GUI, solo CLI, and agent-assisted entry points", async () => {
  const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");
  for (const heading of ["GUI onboarding", "Solo CLI onboarding", "Agent-assisted onboarding"]) {
    assert.match(readme, new RegExp(`### ${heading}`));
  }
  assert.match(readme, /same onboarding core/i);
  assert.match(readme, /--publish-registration/);
  assert.match(readme, /agent[\s\S]*must not add[\s\S]*--publish-registration/i);
  assert.match(readme, /machine-readable JSON/i);
});
