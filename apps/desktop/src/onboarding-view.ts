import { escapeHtml } from "./html.ts";
import { stableComputerId } from "./onboarding-controller.ts";
import { icon } from "./icons.ts";
import { SIX_PACK, sixPackMarkup } from "./instruments.ts";
import { lampIcon } from "./icons.ts";

export type OnboardingProvider = "native" | "apm" | "chezmoi";
export type OnboardingStep = 1 | 2 | 3 | 4;
export type GithubAuthState =
  | { state: "checking" }
  | { state: "signedOut" }
  | { state: "authenticating" }
  | { state: "authenticated"; login: string }
  | { state: "error"; message: string };

export interface OnboardingDraft {
  mode: "create" | "connect";
  repositoryName: string;
  repository: string;
  kitDirectory: string;
  loadout: string;
  projectLoadout: string;
  targetOverride: string;
  computerName: string;
  targetRoot: string;
  provider: OnboardingProvider;
  providerExecutable: string;
  providerVersion: string;
  apmManifest: string;
  apmLockfile: string;
  apmPolicy: string;
  chezmoiSource: string;
  chezmoiConfig: string;
  publishRegistration: boolean;
}

export interface OnboardingViewState {
  step: OnboardingStep;
  auth: GithubAuthState;
  draft: OnboardingDraft;
  message: string;
  submitting: boolean;
}

export function defaultOnboardingDraft(): OnboardingDraft {
  return {
    mode: "create",
    repositoryName: "commonkit",
    repository: "",
    kitDirectory: "",
    loadout: "personal",
    projectLoadout: "",
    targetOverride: "",
    computerName: "workstation",
    targetRoot: "",
    provider: "native",
    providerExecutable: "",
    providerVersion: "",
    apmManifest: "apm.yml",
    apmLockfile: "apm.lock.yaml",
    apmPolicy: "apm-policy.yml",
    chezmoiSource: "home",
    chezmoiConfig: "chezmoi.toml",
    publishRegistration: true,
  };
}

/* The start sequence. Numbers carry real order here: each item gates the next. */
const SEQUENCE = ["Identity", "Station", "Source", "Arm"] as const;

export function onboardingPanel(state: OnboardingViewState): string {
  const sequence = SEQUENCE.map((label, index) => {
    const number = index + 1;
    const status = number < state.step ? "done" : number === state.step ? "active" : "pending";
    const mark = status === "done" ? "SET" : status === "active" ? "◂ NOW" : "";
    return `<li data-state="${status}"${status === "active" ? ' aria-current="step"' : ""}><i>${String(number).padStart(2, "0")}</i>${label}<em>${mark}</em></li>`;
  }).join("");

  const body = state.step === 1 ? identityStep(state)
    : state.step === 2 ? stationStep(state)
    : state.step === 3 ? sourceStep(state)
    : armStep(state);

  return `<section class="startup" aria-busy="${state.submitting}">
    <div class="startup-side">
      <ol class="sequence" aria-label="Start sequence">${sequence}</ol>
      <p class="hint">No telemetry. This computer is not registered, so every instrument is unpowered.</p>
      ${sixPackMarkup(SIX_PACK, true)}
    </div>
    <div>
      <div class="placard"><h1 id="setup-heading" tabindex="-1">${headline(state.step)}</h1><span>Step ${state.step} of 4</span></div>
      ${setupLamps(state)}
      ${body}
      <p class="status-line" role="status" aria-live="polite">${escapeHtml(state.message)}</p>
    </div>
  </section>`;
}

/* The promise stays on screen through the whole sequence, not just at the end. */
function setupLamps(state: OnboardingViewState): string {
  const connected = state.auth.state === "authenticated";
  const lamps: Array<[string, "live" | "caution"]> = [
    [connected ? "GitHub connected" : "GitHub not connected", connected ? "live" : "caution"],
    ["Not registered", "caution"],
    ["Nothing applied", "caution"],
  ];
  return `<div class="annunciator" style="margin-bottom:18px">${lamps
    .map(([text, tone]) => `<p class="lamp" data-state="${tone}">${lampIcon(tone)}<span>${text}</span></p>`)
    .join("")}</div>`;
}

function headline(step: OnboardingStep): string {
  return step === 1 ? "Identity" : step === 2 ? "Station" : step === 3 ? "Source" : "Arm";
}

function identityStep({ auth, draft }: OnboardingViewState): string {
  if (auth.state !== "authenticated") {
    return `<div class="plate-block">
      <div class="plate-head"><h2>GitHub</h2><span>${auth.state === "checking" ? "Checking" : "Signed out"}</span></div>
      <div class="plate-body">
        <p>CommonKit keeps your configuration, never your passwords, in a private repository on GitHub so every computer can start from the same setup.</p>
        <p>Sign-in runs through the GitHub CLI. Your token stays in its native credential store and never reaches this window.</p>
        ${auth.state === "error" ? `<p class="hint bad" role="alert">${escapeHtml(auth.message)}</p>` : ""}
      </div>
    </div>
    <div class="plate-block"><div class="plate-head"><h3>After you connect</h3><span>Three steps</span></div><div class="plate-body"><dl class="readout">
      <dt>Station</dt><dd>Name this computer and pick the one folder CommonKit may manage.</dd>
      <dt>Source</dt><dd>Start empty, or read an APM or chezmoi setup you already keep.</dd>
      <dt>Arm</dt><dd>Review everything, then prepare a plan. Nothing is applied.</dd>
    </dl></div></div>
    <div class="controls split" style="margin-top:16px"><span></span>
      <button type="button" class="engage" id="github-sign-in" ${auth.state === "checking" || auth.state === "authenticating" ? "disabled" : ""}>${auth.state === "authenticating" ? "Waiting for GitHub" : "Connect GitHub"}</button>
    </div>`;
  }
  return `<form id="onboarding-form" data-step="1">
    <div class="plate-block">
      <div class="plate-head"><h2>GitHub</h2><span>${escapeHtml(auth.login)}</span></div>
      <div class="plate-body">
        <fieldset class="selector two"><legend class="sr-only">Starting point</legend>
          ${option("mode", "create", "New kit", "CommonKit creates the private repository for you.", draft.mode === "create")}
          ${option("mode", "connect", "Existing kit", "Connect a CommonKit repository you already have.", draft.mode === "connect")}
        </fieldset>
        ${draft.mode === "create"
          ? textField("repositoryName", "Repository name", draft.repositoryName, `Created private as ${escapeHtml(auth.login)}/${escapeHtml(draft.repositoryName || "commonkit")}.`)
          : textField("repository", "Repository", draft.repository, "Owner and name, for example your-account/commonkit.")}
      </div>
    </div>
    ${actions("Continue", false)}
  </form>`;
}

function stationStep({ draft }: OnboardingViewState): string {
  return `<form id="onboarding-form" data-step="2">
    <div class="plate-block">
      <div class="plate-head"><h2>This computer</h2><span>${escapeHtml(stableComputerId(draft.computerName))}</span></div>
      <div class="plate-body">
        <p>Name this machine and choose the folder CommonKit is allowed to manage.</p>
        ${textField("computerName", "Station name", draft.computerName, `Recorded as ${escapeHtml(stableComputerId(draft.computerName))}.`)}
        ${textField("targetRoot", "Managed root", draft.targetRoot, "Everything CommonKit writes stays inside this folder. An empty one is the safest first run.", "dir")}
        <label class="toggle"><input type="checkbox" id="safe-test-folder" ${draft.targetRoot.endsWith("/CommonKitManaged") ? "checked" : ""}> Use a separate test folder</label>
        <details class="aux"><summary>Local paths and layers</summary><div class="aux-body">
          ${textField("kitDirectory", "Kit checkout", draft.kitDirectory, "Where the private Git checkout lives on this computer.", "dir")}
          ${textField("loadout", "Loadout", draft.loadout, "The portable profile this computer composes from.")}
          ${textField("projectLoadout", "Project layer", draft.projectLoadout, "Optional. Applied after the personal loadout.", false, false)}
          ${textField("targetOverride", "Station layer", draft.targetOverride, "Optional. The final layer, this computer only.", false, false)}
        </div></details>
      </div>
    </div>
    ${actions("Continue")}
  </form>`;
}

function sourceStep({ draft }: OnboardingViewState): string {
  return `<form id="onboarding-form" data-step="3">
    <div class="plate-block">
      <div class="plate-head"><h2>Existing settings</h2><span>${draft.provider === "native" ? "None" : draft.provider === "apm" ? "APM 0.25.0" : "chezmoi 2.70.4"}</span></div>
      <div class="plate-body">
        <p>Start empty, or bring in a setup you already manage. Providers only compute desired state; CommonKit still owns every change to this machine.</p>
        <fieldset class="selector three"><legend class="sr-only">Settings source</legend>
          ${option("provider", "native", "Empty", "Begin with a clean portable kit.", draft.provider === "native")}
          ${option("provider", "apm", "APM", "Read an existing manifest and lockfile.", draft.provider === "apm")}
          ${option("provider", "chezmoi", "chezmoi", "Read an existing source directory.", draft.provider === "chezmoi")}
        </fieldset>
        ${providerDetails(draft)}
      </div>
    </div>
    ${actions("Continue")}
  </form>`;
}

function armStep({ auth, draft, submitting }: OnboardingViewState): string {
  const login = auth.state === "authenticated" ? auth.login : "";
  const repository = draft.mode === "create" ? `${login}/${draft.repositoryName}` : draft.repository;
  const source = draft.provider === "native" ? "Empty kit" : draft.provider === "apm" ? "APM import" : "chezmoi import";
  return `<form id="onboarding-form" data-step="4">
    <div class="plate-block">
      <div class="plate-head"><h2>Before arming</h2><span>Read-only</span></div>
      <div class="plate-body">
        <dl class="readout">
          <dt>Repository</dt><dd>${escapeHtml(repository)}</dd>
          <dt>Kit checkout</dt><dd>${escapeHtml(draft.kitDirectory)}</dd>
          <dt>Station</dt><dd>${escapeHtml(stableComputerId(draft.computerName))}</dd>
          <dt>Managed root</dt><dd>${escapeHtml(draft.targetRoot)}</dd>
          <dt>Loadout</dt><dd>${escapeHtml(draft.loadout)}</dd>
          ${draft.projectLoadout ? `<dt>Project layer</dt><dd>${escapeHtml(draft.projectLoadout)}</dd>` : ""}
          ${draft.targetOverride ? `<dt>Station layer</dt><dd>${escapeHtml(draft.targetOverride)}</dd>` : ""}
          <dt>Source</dt><dd>${escapeHtml(source)}</dd>
        </dl>
      </div>
    </div>
    <div class="notice caution" style="margin-top:16px">${icon.caution()}<p><strong>Nothing is applied by this step.</strong> CommonKit saves the setup locally and prepares a plan. The managed root is not touched until you review that plan and confirm it.</p></div>
    <label class="guard" style="margin-top:16px"><input name="publishRegistration" type="checkbox" value="true" ${draft.publishRegistration ? "checked" : ""} required>
      <span><strong>Publish this station to the repository</strong><small>Creates and pushes a registration commit so your other computers can find this one. This is the only step here that writes to GitHub.</small></span></label>
    ${actions(submitting ? "Preparing plan" : "Prepare plan", true, submitting)}
  </form>`;
}

function providerDetails(draft: OnboardingDraft): string {
  if (draft.provider === "native") return "";
  const fields = draft.provider === "apm"
    ? `${textField("providerExecutable", "APM executable", draft.providerExecutable, "Version 0.25.0 is verified before use.", "file")}
       ${textField("apmManifest", "Manifest", draft.apmManifest, "Relative to the kit checkout.", "file")}
       ${textField("apmLockfile", "Lockfile", draft.apmLockfile, "Relative to the kit checkout.", "file")}
       ${textField("apmPolicy", "Package policy", draft.apmPolicy, "Relative to the kit checkout.", "file")}
       <input name="providerVersion" type="hidden" value="0.25.0">`
    : `${textField("providerExecutable", "chezmoi executable", draft.providerExecutable, "Version 2.70.4 is verified before use.", "file")}
       ${textField("chezmoiSource", "Source directory", draft.chezmoiSource, "Relative to the kit checkout.", "dir")}
       ${textField("chezmoiConfig", "Config", draft.chezmoiConfig, "Relative to the kit checkout.", "file")}
       <input name="providerVersion" type="hidden" value="2.70.4">`;
  return `<details class="aux" open><summary>Provider paths</summary><div class="aux-body">${fields}</div></details>`;
}

function option(name: string, value: string, title: string, detail: string, checked: boolean): string {
  return `<label class="switch"><input type="radio" name="${name}" value="${value}" ${checked ? "checked" : ""}><strong>${title}</strong><small>${detail}</small></label>`;
}

/** `picker`: false, "dir" for a folder, or "file" for an executable. */
function textField(name: string, label: string, value: string, hint: string, picker: false | "dir" | "file" = false, required = true): string {
  const id = `onboarding-${name}`;
  const control = `<input id="${id}" name="${name}" value="${escapeHtml(value)}"${required ? " required" : ""} aria-describedby="${id}-hint">`;
  const attribute = picker === "dir" ? `data-pick-directory="${name}"` : `data-pick="${name}"`;
  const inner = picker
    ? `<div class="field-row">${control}<button type="button" class="standby" ${attribute}>Browse</button></div>`
    : control;
  return `<label class="field" for="${id}"><span>${label}${required ? "" : " <small>optional</small>"}</span>${inner}<small class="hint" id="${id}-hint">${hint}</small></label>`;
}

function actions(next: string, back = true, disabled = false): string {
  return `<div class="controls split" style="margin-top:16px">
    ${back ? `<button class="standby" type="button" id="setup-back">Back</button>` : "<span></span>"}
    <button class="engage" type="submit"${disabled ? " disabled" : ""}>${next}</button>
  </div>`;
}
