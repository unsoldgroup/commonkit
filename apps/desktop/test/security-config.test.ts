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

test("first-run defaults stay native and require no webview path capability", async () => {
  const capability = JSON.parse(await readFile(new URL("capabilities/main.json", root), "utf8"));
  const frontend = await readFile(new URL("../src/main.ts", root), "utf8");
  const backend = await readFile(new URL("src/lib.rs", root), "utf8");
  assert.doesNotMatch(frontend, /@tauri-apps\/api\/path|homeDir\(/);
  assert.doesNotMatch(JSON.stringify(capability.permissions), /core:path/);
  assert.match(backend, /fn onboarding_defaults\(/);
  assert.match(backend, /onboarding_defaults,/);
});

test("background refresh preserves in-progress onboarding input before rerender", async () => {
  const frontend = await readFile(new URL("../src/main.ts", root), "utf8");
  const render = frontend.slice(frontend.indexOf("function render()"), frontend.indexOf("function bindTargetActions"));
  assert.match(render, /captureOnboardingDraft\(\)/);
  assert.ok(render.indexOf("captureOnboardingDraft()") < render.indexOf("app.innerHTML"));
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

test("macOS shows the Dock icon only while the main window is open", async () => {
  const source = await readFile(new URL("src/lib.rs", root), "utf8");
  const showWindow = source.slice(
    source.indexOf("fn show_main_window"),
    source.indexOf("fn show_route"),
  );
  const hideWindow = source.slice(
    source.indexOf("fn hide_main_window"),
    source.indexOf("fn show_route"),
  );
  const windowEvents = source.slice(source.indexOf(".on_window_event"));
  const setup = source.slice(
    source.indexOf(".setup(move |app|"),
    source.indexOf(".on_window_event"),
  );

  assert.match(
    showWindow,
    /set_activation_policy\(tauri::ActivationPolicy::Regular\)/,
  );
  assert.ok(
    showWindow.indexOf("ActivationPolicy::Regular") <
      showWindow.indexOf("window.show()"),
    "the Dock icon must be restored before showing the window",
  );
  assert.match(
    showWindow,
    /window\.inner_size\(\)[\s\S]*?window\s*\.set_size\(/,
    "existing installations must expand a restored undersized window before showing it",
  );
  assert.match(
    hideWindow,
    /set_activation_policy\(tauri::ActivationPolicy::Accessory\)/,
  );
  assert.ok(
    hideWindow.indexOf("window.hide()") <
      hideWindow.indexOf("ActivationPolicy::Accessory"),
    "closing the window must hide it before returning to menu-bar-only mode",
  );
  assert.match(windowEvents, /hide_main_window\(window\)/);
  assert.doesNotMatch(windowEvents, /let _ = window\.hide\(\)/);
  assert.doesNotMatch(
    windowEvents,
    /let _ = window[\s\S]*?set_activation_policy/,
  );
  assert.match(setup, /arg == "--minimized"/);
  assert.match(
    setup,
    /if !start_minimized \{[\s\S]*?show_main_window/,
  );
  assert.match(
    windowEvents,
    /RunEvent::Reopen[\s\S]*?show_main_window/,
    "reopening a tray-only app must restore its hidden window as well as its Dock icon",
  );
});

test("macOS Dock lifecycle smoke verifies open, close, and reopen against OS state", async () => {
  const smoke = await readFile(
    new URL("../../../scripts/macos-dock-lifecycle-smoke.sh", import.meta.url),
    "utf8",
  );

  assert.match(smoke, /NSRunningApplication/);
  assert.match(smoke, /activationPolicy/);
  assert.match(smoke, /process "Dock"/);
  assert.match(smoke, /process "CommonKit"/);
  assert.match(smoke, /open -a CommonKit/);
  assert.match(smoke, /Open CommonKit/);
  assert.match(smoke, /expected Regular/);
  assert.match(smoke, /expected Accessory/);
});

test("tray is a read-only status surface with only open and quit actions", async () => {
  const source = await readFile(new URL("src/lib.rs", root), "utf8");
  const setup = source.slice(source.indexOf(".setup(move |app|"), source.indexOf(".on_window_event"));
  for (const removed of [
    "quick-fetch", "quick-plan", "quick-verify", "quick-snapshot",
    "review", "manage-relay", "manage-snapshots", "refresh",
  ]) {
    assert.doesNotMatch(setup, new RegExp(`\"${removed}\"`));
  }
  assert.match(setup, /MenuItem::with_id\(app,\s*"open"/);
  assert.match(setup, /MenuItem::with_id\(app,\s*"quit"/);
});

test("manual release policy requires native desktop and daemon lifecycle evidence", async () => {
  const release = await readFile(new URL("../../../docs/RELEASING.md", import.meta.url), "utf8");
  assert.match(release, /trusted release workstation/i);
  assert.match(release, /manually invoked platform runner/i);
  assert.match(release, /install → reconcile → verify → snapshot\/restore → recover → update →\s+uninstall/);
  assert.match(release, /scripts\/installed-lifecycle\.sh/);
});

test("installed lifecycle atomically reloads an already-running daemon after onboarding", async () => {
  const lifecycle = (await readFile(new URL("../../../scripts/installed-lifecycle.sh", import.meta.url), "utf8"))
    .replaceAll("\r\n", "\n");
  const firstStart = lifecycle.search(/(?:^|\n)start_daemon\n/);
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

test("manual release builds bundle target-matched sidecars before Tauri packaging", async () => {
  const release = await readFile(new URL("../../../docs/RELEASING.md", import.meta.url), "utf8");
  const bundle = release.indexOf("scripts/stage-desktop-sidecars.sh");
  const tauri = release.indexOf("Tauri build command");
  assert.ok(bundle >= 0 && tauri > bundle, "sidecars must be staged before the installer is built");
  assert.match(release, /does not use GitHub Actions/i);
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

test("credential provisioning reviews a redacted plan before applying its exact plan id", async () => {
  const frontend = await readFile(new URL("../src/main.ts", root), "utf8");
  const api = await readFile(new URL("../src/api.ts", root), "utf8");
  const flow = frontend.slice(
    frontend.indexOf('document.querySelector("#credential-apply")'),
    frontend.indexOf('document.querySelector("#credential-verify")'),
  );
  const plan = flow.indexOf("credentialPlan([id])");
  const review = flow.indexOf("confirmAction", plan);
  const apply = flow.indexOf("credentialApply(plan.planId", review);
  assert.ok(plan >= 0 && review > plan && apply > review);
  assert.match(api, /credentialApply:\s*\(planId: string/);
  assert.doesNotMatch(api, /credentialApply:\s*\(destinationIds/);
});

test("operator workflows use inline controls instead of technical browser prompts", async () => {
  const source = await readFile(new URL("../src/main.ts", root), "utf8");
  assert.doesNotMatch(source, /window\.prompt/);
  assert.doesNotMatch(source, /Configured credential destination ID/);
});

test("application and menu-bar icons share the same Ck monogram geometry", async () => {
  const packageJson = JSON.parse(await readFile(new URL("../package.json", root), "utf8"));
  const tauriConfig = JSON.parse(
    await readFile(new URL("../src-tauri/tauri.conf.json", root), "utf8"),
  );
  const tray = await readFile(new URL("../src-tauri/icons/tray-ck.svg", root), "utf8");
  const app = await readFile(new URL("../src-tauri/icons/app-icon.svg", root), "utf8");
  const paths = [...tray.matchAll(/<path d="([^"]+)"/g)].map((match) => match[1]);

  assert.equal(paths.length, 2);
  for (const path of paths) assert.match(app, new RegExp(path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.match(app, /fill="#FFFFFF"/);
  assert.match(app, /fill="#1F6F50"/);
  assert.equal(packageJson.scripts.icons, "tauri icon src-tauri/icons/app-icon.svg");
  assert.deepEqual(tauriConfig.bundle.icon, [
    "icons/32x32.png",
    "icons/128x128.png",
    "icons/128x128@2x.png",
    "icons/icon.icns",
    "icons/icon.ico",
  ]);
});
