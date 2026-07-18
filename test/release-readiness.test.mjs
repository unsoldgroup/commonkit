import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);

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

  assert.doesNotMatch(workflow, /if:\s*\$\{\{\s*false\s*\}\}/);
  assert.doesNotMatch(workflow, /Disabled contract/);
  assert.match(workflow, /tauri-apps\/tauri-action/);
  assert.match(workflow, /build-release-cli\.sh/);
  assert.match(workflow, /upload-artifact/);
  assert.match(workflow, /libwebkit2gtk-4\.1-dev/);
  assert.match(workflow, /Import-PfxCertificate/);
  assert.match(workflow, /WINDOWS_CERTIFICATE_THUMBPRINT/);
  assert.match(workflow, /tauri-apps\/tauri-action@v1/);
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
    await execFileAsync(process.execPath, [
      new URL("../scripts/assemble-updater-manifest.mjs", import.meta.url).pathname,
      root,
      "v1.2.3",
    ], { env: { ...process.env, COMMONKIT_RELEASE_DOWNLOAD_BASE: "https://releases.example/v1.2.3" } });
    const manifest = JSON.parse(await readFile(join(root, "latest.json"), "utf8"));
    assert.deepEqual(Object.keys(manifest.platforms).sort(), [
      "darwin-aarch64",
      "darwin-x86_64",
      "linux-x86_64",
      "windows-x86_64",
    ]);
    assert.equal(manifest.platforms["darwin-aarch64"].signature, "mac-signature");
    assert.deepEqual(
      manifest.platforms["darwin-aarch64"],
      manifest.platforms["darwin-x86_64"],
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("release lifecycle matrix exercises install update and uninstall", async () => {
  const workflow = await read(".github/workflows/release-lifecycle.yml");

  assert.match(workflow, /macos-latest/);
  assert.match(workflow, /ubuntu-latest/);
  assert.match(workflow, /windows-latest/);
  assert.match(workflow, /install/);
  assert.match(workflow, /update/);
  assert.match(workflow, /uninstall/);
  assert.doesNotMatch(workflow, /if:\s*\$\{\{\s*false\s*\}\}/);
  assert.doesNotMatch(workflow, /run:\s*echo/);
  assert.match(workflow, /gh release download/);
  assert.match(workflow, /hdiutil attach/);
  assert.match(workflow, /dpkg -i/);
  assert.match(workflow, /Start-Process.*\/S/);
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
