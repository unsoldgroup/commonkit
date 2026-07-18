import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("release workflow covers signed desktop and standalone CLI artifacts", async () => {
  const workflow = await read(".github/workflows/release.yml");

  for (const value of [
    "universal-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "AppImage",
    "deb",
    "nsis",
    "APPLE_CERTIFICATE",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "WINDOWS_CERTIFICATE",
    "WINDOWS_CERTIFICATE_PASSWORD",
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
    "syft",
    "cosign sign-blob",
    "SHA256SUMS",
    "latest.json",
  ]) {
    assert.match(workflow, new RegExp(value.replaceAll("-", "\\-")), value);
  }
});

test("release inputs are validated before packaging", async () => {
  const workflow = await read(".github/workflows/release.yml");
  const validator = await read("scripts/prepare-release-config.mjs");

  assert.match(workflow, /prepare-release-config\.mjs/);
  assert.match(workflow, /verify-release-assets\.mjs/);
  assert.match(validator, /TAURI_UPDATER_PUBLIC_KEY/);
  assert.match(validator, /COMMONKIT_UPDATE_ENDPOINT/);
  assert.match(validator, /process\.exitCode = 1/);
});

test("release lifecycle matrix exercises install update and uninstall", async () => {
  const workflow = await read(".github/workflows/release-lifecycle.yml");

  assert.match(workflow, /macos-latest/);
  assert.match(workflow, /ubuntu-latest/);
  assert.match(workflow, /windows-latest/);
  assert.match(workflow, /install/);
  assert.match(workflow, /update/);
  assert.match(workflow, /uninstall/);
});

test("release and third-party notice policies are explicit", async () => {
  const [release, notices, migration, support] = await Promise.all([
    read("docs/RELEASING.md"),
    read("THIRD-PARTY-NOTICES.md"),
    read("docs/MIGRATION.md"),
    read("docs/SUPPORT-MATRIX.md"),
  ]);

  assert.match(release, /signed and notarized universal macOS/i);
  assert.match(release, /fail closed/i);
  assert.match(notices, /chezmoi/i);
  assert.match(notices, /not redistributed/i);
  assert.match(migration, /update channel/i);
  assert.match(support, /standalone CLI/i);
});
