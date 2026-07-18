import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../src-tauri/", import.meta.url);

test("desktop capabilities expose no shell, process, filesystem, or arbitrary HTTP access", async () => {
  const capability = JSON.parse(await readFile(new URL("capabilities/main.json", root), "utf8"));
  const permissions = JSON.stringify(capability.permissions);
  for (const denied of ["shell", "process", "fs:", "http:"]) assert.ok(!permissions.includes(denied), denied);
  assert.deepEqual(capability.windows, ["main"]);
});

test("desktop is single-window, CSP-bound, and emits updater artifacts", async () => {
  const config = JSON.parse(await readFile(new URL("tauri.conf.json", root), "utf8"));
  assert.equal(config.app.windows.length, 1);
  assert.match(config.app.security.csp, /default-src 'self'/);
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.equal(config.app.withGlobalTauri, false);
});

test("updater commands separate inspection from explicitly confirmed installation", async () => {
  const source = await readFile(new URL("src/lib.rs", root), "utf8");
  assert.match(source, /struct UpdateSummary/);
  assert.match(source, /update\.body/);
  assert.match(source, /install_update/);
  assert.match(source, /confirmed:\s*bool/);
  assert.match(source, /download_and_install/);
  assert.doesNotMatch(source, /check_for_update[\s\S]{0,500}download_and_install/);
});
