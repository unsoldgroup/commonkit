import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../src-tauri/", import.meta.url);

test("desktop capabilities expose no shell, process, filesystem, or arbitrary HTTP access", async () => {
  const capability = JSON.parse(await readFile(new URL("capabilities/main.json", root), "utf8"));
  const permissions = JSON.stringify(capability.permissions);
  for (const denied of ["shell", "process", "fs:", "http:"]) assert.ok(!permissions.includes(denied), denied);
  assert.ok(capability.permissions.includes("dialog:allow-open"));
  assert.deepEqual(capability.windows, ["main"]);
});

test("desktop is single-window, CSP-bound, and emits updater artifacts", async () => {
  const config = JSON.parse(await readFile(new URL("tauri.conf.json", root), "utf8"));
  const source = await readFile(new URL("src/lib.rs", root), "utf8");
  assert.equal(config.app.windows.length, 1);
  assert.match(config.app.security.csp, /default-src 'self'/);
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.equal(config.app.withGlobalTauri, false);
  assert.deepEqual(config.plugins?.updater, {
    pubkey: "",
    endpoints: [],
  });
  assert.match(source, /set_activation_policy\(tauri::ActivationPolicy::Accessory\)/);
  assert.deepEqual(config.bundle.externalBin, [
    "binaries/commonkit",
    "binaries/commonkitd",
    "binaries/commonkit-target-helper",
  ]);
});

test("published installer lifecycle launches the installed desktop and waits for daemon health", async () => {
  const workflow = await readFile(new URL("../../../.github/workflows/release-lifecycle.yml", import.meta.url), "utf8");
  assert.match(workflow, /COMMONKIT_DESKTOP_SMOKE_REPORT/);
  assert.match(workflow, /Applications\/CommonKit\.app\/Contents\/MacOS\/commonkit-desktop/);
  assert.match(workflow, /commonkit-desktop-smoke\.json/);
  assert.match(workflow, /serviceState/);
});

test("installed lifecycle atomically reloads an already-running daemon after onboarding", async () => {
  const lifecycle = await readFile(new URL("../../../scripts/installed-lifecycle.sh", import.meta.url), "utf8");
  const firstStart = lifecycle.indexOf("trap stop_daemon EXIT\nstart_daemon");
  const initialized = lifecycle.indexOf('"$commonkit" init connect');
  const reload = lifecycle.indexOf('daemon reload-domains --confirmed');
  const firstPlan = lifecycle.indexOf('"$commonkit" sync --confirmed');
  assert.ok(firstStart >= 0 && firstStart < initialized, "daemon must start before onboarding writes headless.json");
  assert.ok(reload > initialized && reload < firstPlan, "running daemon must adopt domains before the first plan is used");
  assert.equal(lifecycle.indexOf('daemon restart', initialized), -1, "onboarding must not replace an attached daemon");
});

test("desktop preflights and reloads through the authenticated daemon API regardless of process ownership", async () => {
  const source = await readFile(new URL("src/lib.rs", root), "utf8");
  const onboarding = source.slice(source.indexOf("fn onboarding_initialize"), source.indexOf("impl ServiceClient"));
  const preflight = onboarding.indexOf("reqwest::Method::GET");
  const initialize = onboarding.indexOf("let result = initialize(");
  const reload = onboarding.indexOf("reqwest::Method::POST");
  assert.ok(preflight >= 0 && preflight < initialize);
  assert.ok(reload > initialize);
  assert.doesNotMatch(onboarding, /reload_owned_after_onboarding|ExternalServiceReloadRequired/);
});

test("release builds bundle target-matched CLI and daemon before Tauri packaging", async () => {
  const workflow = await readFile(new URL("../../../.github/workflows/release.yml", import.meta.url), "utf8");
  const bundle = workflow.indexOf("Stage desktop sidecars");
  const tauri = workflow.indexOf("Build signed and notarized desktop bundles");
  assert.ok(bundle >= 0 && tauri > bundle, "sidecars must be staged before the installer is built");
  assert.match(workflow, /scripts\/stage-desktop-sidecars\.sh/);
});

test("updater commands separate inspection from explicitly confirmed installation", async () => {
  const source = await readFile(new URL("src/lib.rs", root), "utf8");
  assert.match(source, /struct UpdateSummary/);
  assert.match(source, /update\.body/);
  assert.match(source, /install_update/);
  assert.match(source, /confirmed:\s*bool/);
  assert.match(source, /download_and_install/);
  assert.doesNotMatch(source, /check_for_update[\s\S]{0,500}download_and_install/);
  assert.doesNotMatch(source, /"installedVersion"\s*:\s*expected/);
  assert.match(source, /"desktopVersion"\s*:\s*env!\("CARGO_PKG_VERSION"\)/);
  assert.match(source, /updater_builder\(\)/);
  assert.match(source, /on_before_exit/);
  assert.match(source, /"updaterExitPrepared"\s*:\s*true/);
  assert.match(source, /cleanup_before_exit/);
  const lifecycle = source.slice(source.indexOf("async fn run_automated_update_lifecycle"), source.indexOf("fn show_main_window"));
  const download = lifecycle.indexOf(".download(");
  const marker = lifecycle.indexOf("write_update_report");
  const install = lifecycle.indexOf(".install(");
  assert.ok(download >= 0 && marker > download && install > marker, "verified download must precede durable handoff and installer launch");
  assert.doesNotMatch(lifecycle, /let _ = write_update_report/);
  assert.match(source, /expected\s*==\s*env!\("CARGO_PKG_VERSION"\)/);
});
