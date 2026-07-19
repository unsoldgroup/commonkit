import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("updater manifest carries generated release notes for the consent UI", async () => {
  const root = await mkdtemp(join(tmpdir(), "commonkit-manifest-"));
  for (const name of ["CommonKit.app.tar.gz", "CommonKit.AppImage", "CommonKit.nsis.zip"]) {
    await writeFile(join(root, name), name);
    await writeFile(join(root, `${name}.sig`), `signature-${name}`);
  }
  const notes = join(root, "release-notes.md");
  await writeFile(notes, "Security fixes and recovery improvements.\n");
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("../../../scripts/assemble-updater-manifest.mjs", import.meta.url)), root, "v0.2.0", notes], {
    cwd: fileURLToPath(new URL(".", import.meta.url)),
    env: { ...process.env, COMMONKIT_RELEASE_DOWNLOAD_BASE: "https://releases.example.test/v0.2.0" },
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(await readFile(join(root, "latest.json"), "utf8"));
  assert.equal(manifest.version, "0.2.0");
  assert.equal(manifest.notes, "Security fixes and recovery improvements.");
});
