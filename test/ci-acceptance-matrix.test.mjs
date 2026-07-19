import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workflowUrl = new URL("../.github/workflows/ci.yml", import.meta.url);

test("CI exercises the v1 contracts on every supported desktop platform", async () => {
  const workflow = await readFile(workflowUrl, "utf8");

  for (const os of ["macos-latest", "ubuntu-latest", "windows-latest"]) {
    assert.match(workflow, new RegExp(`\\b${os}\\b`));
  }
  assert.match(workflow, /node-version:\s*24/);
  assert.match(workflow, /pnpm --dir apps\/desktop test/);
  assert.match(workflow, /apps\/desktop\/src-tauri\/Cargo\.toml/);
  assert.match(workflow, /CARGO_INCREMENTAL:\s*0/);
  assert.match(workflow, /native_windows_credential_manager_round_trip_is_target_scoped/);
  assert.match(workflow, /native_windows_private_path_acl_is_enforced_and_verified/);
});

test("provider gates use exact releases and verify downloaded bytes", async () => {
  const workflow = await readFile(workflowUrl, "utf8");

  assert.match(workflow, /APM_VERSION:\s*["']?0\.25\.0/);
  assert.match(workflow, /CHEZMOI_VERSION:\s*["']?2\.70\.4/);
  assert.match(workflow, /7a4281ef915e9609f8841eced19285bf2dcf702862302171f7e893c30e1b3244/);
  assert.match(workflow, /1d9b3b44ffe031c331f4dfe33694bbb80d10d16165ca053110b96dbeb7365943/);
  assert.doesNotMatch(workflow, /apm-darwin|chezmoi-darwin|sandbox-exec/);
  assert.match(workflow, /checksum_pinned_apm_release_materializes_without_touching_live_target/);
  assert.match(workflow, /real_chezmoi_release_materializes_supported_fixtures_deterministically/);
  assert.doesNotMatch(workflow, /continue-on-error:\s*true/);
});
