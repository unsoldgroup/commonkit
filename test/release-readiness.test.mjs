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
    const notes = join(root, "release-notes.md");
    await writeFile(notes, "Security and reliability improvements.");
    await execFileAsync(process.execPath, [
      new URL("../scripts/assemble-updater-manifest.mjs", import.meta.url).pathname,
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
  assert.match(workflow, /commonkitd-macos-universal/);
  assert.match(workflow, /commonkitd-linux-x86_64/);
  assert.match(workflow, /commonkitd-windows-x86_64\.exe/);
  assert.match(workflow, /installed-lifecycle\.sh/);
  assert.match(workflow, /COMMONKIT_DESKTOP_UPDATE_REPORT/);
  assert.match(workflow, /COMMONKIT_DESKTOP_UPDATE_EXPECTED_VERSION/);
  assert.match(workflow, /updatedByTauri/);
  assert.match(workflow, /updaterExitPrepared/);
  assert.match(workflow, /Wait for the Windows updater to replace the installed executable/);
  assert.match(workflow, /desktopVersion/);
  assert.match(workflow, /desktopExecutable/);
  assert.match(workflow, /previous_hash/);
  assert.match(workflow, /updated_hash/);
  assert.match(workflow, /CommonKit\.AppImage[\s\S]*COMMONKIT_DESKTOP_SMOKE_REPORT/);
  assert.doesNotMatch(workflow, /installedVersion/);
  assert.doesNotMatch(workflow, /dpkg -L[^\n]*commonkit-desktop/);
  assert.doesNotMatch(workflow, /Update macOS by installing current signed release/);
  assert.doesNotMatch(workflow, /Update Linux by installing current signed Debian release/);
  assert.doesNotMatch(workflow, /Update Windows by installing current signed NSIS release/);
});

test("CI exercises an unsigned installed CLI and daemon lifecycle on every OS", async () => {
  const workflow = await read(".github/workflows/ci.yml");
  const harness = await read("scripts/installed-lifecycle.sh");

  assert.match(workflow, /unsigned-installed-lifecycle/);
  assert.match(workflow, /cargo install --locked --path crates\/commonkit-cli/);
  assert.match(workflow, /cargo install --locked --path crates\/commonkit-service/);
  assert.match(workflow, /commonkit-target-helper/);
  assert.match(workflow, /commonkitd/);
  assert.match(workflow, /installed-lifecycle\.sh/);
  assert.match(workflow, /commonkit-snapshot-recovery-fixture/);
  assert.doesNotMatch(workflow, /commonkit-snapshots --test durable_restore/);
  assert.match(harness, /"\$commonkit" init connect/);
  assert.match(harness, /"\$commonkit" sync --confirmed/);
  assert.match(harness, /"\$commonkit" apply/);
  assert.match(harness, /"\$commonkit" verify/);
  assert.match(harness, /"\$commonkit" snapshots create/);
  assert.match(harness, /"\$commonkit" snapshots restore/);
  assert.match(harness, /"\$recovery_fixture" interrupt/);
  assert.match(harness, /"\$recovery_fixture" recover/);
  assert.match(harness, /"\$commonkit" relay status/);
  assert.match(harness, /ssh/);
  assert.match(harness, /"\$target_helper" --stdio-v1/);
  assert.doesNotMatch(harness, /target-helper.*\.mjs/);
  for (const command of ["install", "start", "status", "restart", "uninstall"]) {
    assert.match(harness, new RegExp(`"\\$commonkit" daemon ${command}`));
  }
  assert.match(workflow, /test:compatibility/);
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

test("CI isolates the real APM integration behind a checksum-verified provider gate", async () => {
  const [workflow, integration] = await Promise.all([
    read(".github/workflows/ci.yml"),
    read("crates/commonkit-adapters/tests/apm_real_provider.rs"),
  ]);
  const exactTest = "checksum_pinned_apm_release_materializes_without_touching_live_target";

  // The ordinary workspace matrix must remain runnable without downloading a
  // provider, while the real integration remains an explicit required CI test.
  assert.match(integration, /#\[ignore\s*=\s*"requires checksum-verified APM 0\.25\.0 binary"\]/);
  assert.match(integration, /COMMONKIT_APM_025_BIN/);

  for (const job of ["providers-unix:", "providers-windows:"]) {
    const start = workflow.indexOf(job);
    assert.notEqual(start, -1, `${job} missing`);
    const remainder = workflow.slice(start + job.length);
    const nextJobMatch = /\n  [a-z][a-z0-9-]+:\n/.exec(remainder);
    const nextJob = nextJobMatch
      ? start + job.length + nextJobMatch.index
      : -1;
    const body = workflow.slice(start, nextJob === -1 ? undefined : nextJob);
    const checksum = body.search(/shasum -a 256 --check|Get-FileHash/);
    const env = body.indexOf("COMMONKIT_APM_025_BIN");
    const exact = body.indexOf(
      `apm_real_provider ${exactTest} -- --ignored --exact`,
    );
    assert.ok(checksum >= 0, `${job} must verify the downloaded archive`);
    assert.ok(env > checksum, `${job} must export only the verified executable`);
    assert.ok(exact > env, `${job} must run the exact ignored integration after verification`);
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
  assert.match(release, /fail closed/i);
  assert.match(notices, /chezmoi/i);
  assert.match(notices, /not redistributed/i);
  assert.match(migration, /update channel/i);
  assert.match(support, /standalone CLI/i);
});
