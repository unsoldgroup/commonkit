import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("manual release policy covers signed desktop and standalone CLI artifacts", async () => {
  const release = await read("docs/RELEASING.md");
  for (const value of [
    "universal macOS",
    "Windows NSIS",
    "Linux AppImage",
    "AppImage",
    "APPLE_CERTIFICATE",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "WINDOWS_CERTIFICATE",
    "WINDOWS_CERTIFICATE_PASSWORD",
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
    "SPDX SBOM",
  ]) {
    assert.match(release, new RegExp(value.replaceAll("-", "\\-"), "i"), value);
  }
  assert.match(release, /scripts\/build-release-cli\.sh/);
  assert.match(release, /scripts\/verify-release-assets\.mjs/);
  assert.match(release, /fails? closed/i);
  assert.match(release, /does not use GitHub Actions/i);
});

test("release inputs are validated before packaging", async () => {
  const release = await read("docs/RELEASING.md");
  const validator = await read("scripts/prepare-release-config.mjs");

  assert.match(release, /prepare-release-config\.mjs/);
  assert.match(release, /verify-release-assets\.mjs/);
  assert.match(validator, /TAURI_UPDATER_PUBLIC_KEY/);
  assert.match(validator, /COMMONKIT_UPDATE_ENDPOINT/);
  assert.match(validator, /process\.exitCode = 1/);
  assert.match(validator, /COMMONKIT_UPDATE_ENDPOINT must use HTTPS/);
});

test("release download URLs are bound to the repository and exact release tag", async () => {
  const [release, updater] = await Promise.all([
    read("docs/RELEASING.md"),
    read("scripts/assemble-updater-manifest.mjs"),
  ]);
  assert.match(release, /exact version and artifact digests/i);
  assert.match(updater, /COMMONKIT_RELEASE_DOWNLOAD_BASE/);
  assert.match(updater, /must be HTTPS/);
});

test("release automation performs real assembly and fails closed", async () => {
  const [cli, collect, checksum, updater] = await Promise.all([
    read("scripts/build-release-cli.sh"),
    read("scripts/collect-release-assets.sh"),
    read("scripts/checksum-release-assets.sh"),
    read("scripts/assemble-updater-manifest.mjs"),
  ]);
  assert.match(cli, /lipo -create/);
  assert.match(cli, /cargo build --release --locked/);
  assert.match(collect, /no .* desktop release payloads found/);
  assert.match(checksum, /shasum -a 256/);
  assert.match(updater, /updater signature missing/);
  assert.match(updater, /COMMONKIT_RELEASE_DOWNLOAD_BASE must be HTTPS/);
  assert.match(updater, /darwin-aarch64/);
  assert.match(updater, /darwin-x86_64/);
  assert.doesNotMatch(updater, /darwin-universal/);
});

test("updater manifest uses Tauri platform keys and signed payloads", async () => {
  const root = await mkdtemp(join(tmpdir(), "commonkit-updater-"));
  try {
    for (const [name, contents] of [
      ["CommonKit.app.tar.gz", "mac"],
      ["CommonKit.app.tar.gz.sig", "mac-signature"],
      ["CommonKit.AppImage", "linux"],
      ["CommonKit.AppImage.sig", "linux-signature"],
      ["CommonKit.nsis.zip", "windows"],
      ["CommonKit.nsis.zip.sig", "windows-signature"],
    ]) await writeFile(join(root, name), contents);
    const notes = join(root, "release-notes.md");
    await writeFile(notes, "Security and reliability improvements.");
    await execFileAsync(process.execPath, [
      fileURLToPath(new URL("../scripts/assemble-updater-manifest.mjs", import.meta.url)),
      root,
      "v1.2.3",
      notes,
    ], { env: { ...process.env, COMMONKIT_RELEASE_DOWNLOAD_BASE: "https://releases.example/v1.2.3" } });
    const manifest = JSON.parse(await readFile(join(root, "latest.json"), "utf8"));
    assert.deepEqual(Object.keys(manifest.platforms).sort(), [
      "darwin-aarch64",
      "darwin-x86_64",
      "linux-x86_64",
      "windows-x86_64",
    ]);
    assert.equal(manifest.platforms["darwin-aarch64"].signature, "mac-signature");
    assert.equal(manifest.notes, "Security and reliability improvements.");
    assert.deepEqual(
      manifest.platforms["darwin-aarch64"],
      manifest.platforms["darwin-x86_64"],
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("manual release policy requires native install update and uninstall evidence", async () => {
  const release = await read("docs/RELEASING.md");
  for (const value of ["macOS", "Linux", "Windows", "install", "update", "uninstall"]) {
    assert.match(release, new RegExp(value, "i"));
  }
  assert.match(release, /two published versions/i);
  assert.match(release, /artifact digests, commands, and results/i);
  assert.match(release, /scripts\/installed-lifecycle\.sh/);
});

test("manual harness exercises an unsigned installed CLI and daemon lifecycle", async () => {
  const harness = await read("scripts/installed-lifecycle.sh");

  assert.match(harness, /"\$commonkit" init connect/);
  assert.match(harness, /"\$commonkit" sync --confirmed/);
  assert.match(harness, /"\$commonkit" apply/);
  assert.match(harness, /"\$commonkit" verify/);
  assert.match(harness, /apply_run_id=.*runId/);
  assert.match(harness, /"\$commonkit" rollback "\$apply_run_id" --confirmed/);
  assert.match(harness, /commonkit\.layer-content\.v1/);
  for (const layer of [
    "public-base",
    "organization-policy",
    "personal",
    "project-web",
    "target-local",
  ]) {
    assert.match(harness, new RegExp(`writeLayer\\("${layer}"`));
  }
  assert.match(harness, /--project-loadout project-web/);
  assert.match(harness, /--target-override target-local/);
  assert.match(harness, /"\$commonkit" targets plan local secondary --confirmed/);
  assert.match(harness, /"\$commonkit" targets verify local secondary/);
  assert.doesNotMatch(harness, /zero_digest/);
  assert.match(harness, /test ! -e "\$scratch\/target\/portable\/editor\.conf"/);
  assert.doesNotMatch(harness, /reapply_plan_id/);
  assert.match(harness, /"\$commonkit" snapshots create/);
  assert.match(harness, /"\$commonkit" snapshots restore/);
  assert.match(harness, /"\$recovery_fixture" interrupt/);
  assert.match(harness, /"\$recovery_fixture" recover/);
  assert.match(harness, /"\$commonkit" relay status/);
  assert.match(harness, /selected_target_drifted/);
  assert.match(harness, /scheduled_target_digest/);
  assert.match(harness, /ssh/);
  assert.match(harness, /"\$target_helper" --stdio-v1/);
  assert.doesNotMatch(harness, /target-helper.*\.mjs/);
  for (const command of ["install", "start", "status", "restart", "uninstall"]) {
    assert.match(harness, new RegExp(`"\\$commonkit" daemon ${command}`));
  }
});

test("eight-flow qualification binds native evidence to commit and binary digests", async () => {
  const qualifier = await read("scripts/qualify-eight-flows.sh");

  assert.match(qualifier, /installed-lifecycle\.sh/);
  assert.match(qualifier, /COMMONKIT_QUALIFICATION_COMMIT/);
  assert.match(qualifier, /createHash\("sha256"\)/);
  assert.match(qualifier, /process\.platform/);
  assert.match(qualifier, /process\.arch/);
  assert.match(qualifier, /result.*passed/s);
  assert.doesNotMatch(qualifier, /set -x/);
});

test("committed native eight-flow evidence is complete and content-addressed", async () => {
  for (const [path, platform, architecture] of [
    ["docs/evidence/eight-flow-macos-arm64-e25785d.json", "darwin", "arm64"],
    ["docs/evidence/eight-flow-linux-x64-e98dee5.json", "linux", "x64"],
  ]) {
    const evidence = JSON.parse(await read(path));
    assert.match(evidence.commit, /^[0-9a-f]{40}$/);
    assert.equal(evidence.platform, platform);
    assert.equal(evidence.architecture, architecture);
    assert.equal(evidence.result, "passed");
    assert.deepEqual(
      Object.keys(evidence.binaries).sort(),
      [
        "commonkit",
        "commonkit-snapshot-recovery-fixture",
        "commonkit-target-helper",
        "commonkitd",
      ],
    );
    for (const digest of Object.values(evidence.binaries)) {
      assert.match(digest, /^sha256:[0-9a-f]{64}$/);
    }
  }
});

test("release artifacts include and verify the target helper on every platform", async () => {
  const [builder, verifier, desktop] = await Promise.all([
    read("scripts/build-release-cli.sh"),
    read("scripts/verify-release-assets.mjs"),
    read("apps/desktop/src-tauri/tauri.conf.json"),
  ]);
  for (const source of [builder, verifier, desktop]) {
    assert.match(source, /commonkit-target-helper/);
  }
  for (const suffix of ["macos-universal", "linux-x86_64", "windows-x86_64\\.exe"]) {
    assert.match(verifier, new RegExp(`commonkit-target-helper-${suffix}`));
  }
});

test("manual provider validation is bound to checksum-verified releases", async () => {
  const [providerLock, integration] = await Promise.all([
    read("providers/provider-release-lock.json"),
    read("crates/commonkit-adapters/tests/apm_real_provider.rs"),
  ]);
  const exactTest = "checksum_pinned_apm_release_materializes_without_touching_live_target";

  assert.match(integration, /#\[ignore\s*=\s*"requires checksum-verified APM 0\.25\.0 binary"\]/);
  assert.match(integration, /COMMONKIT_APM_025_BIN/);
  assert.match(integration, new RegExp(exactTest));
  const lock = JSON.parse(providerLock);
  assert.equal(lock.apm.version, "0.25.0");
  assert.equal(lock.chezmoi.version, "2.70.4");
  for (const provider of Object.values(lock)) {
    for (const [platform, entry] of Object.entries(provider)) {
      if (platform === "version") continue;
      assert.match(entry.sha256, /^[a-f0-9]{64}$/);
      assert.ok(entry.asset.length > 0);
    }
  }
});

test("release and third-party notice policies are explicit", async () => {
  const [release, notices, migration, support] = await Promise.all([
    read("docs/RELEASING.md"),
    read("THIRD-PARTY-NOTICES.md"),
    read("docs/MIGRATION.md"),
    read("docs/SUPPORT-MATRIX.md"),
  ]);

  assert.match(release, /signed and notarized universal macOS/i);
  assert.match(release, /fails? closed/i);
  assert.match(notices, /chezmoi/i);
  assert.match(notices, /not redistributed/i);
  assert.match(migration, /update channel/i);
  assert.match(support, /standalone CLI/i);
});
