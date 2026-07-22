import { escapeHtml } from "./html.ts";

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

const steps = ["GitHub", "This computer", "Existing settings", "Review"] as const;

export function onboardingPanel(state: OnboardingViewState): string {
  const progress = `<ol class="setup-progress" aria-label="Setup progress">${steps.map((label, index) => {
    const number = index + 1;
    return `<li${state.step === number ? ` aria-current="step"` : ""}><span>${number}</span>${label}</li>`;
  }).join("")}</ol>`;
  const content = state.step === 1 ? githubStep(state) : state.step === 2 ? computerStep(state) : state.step === 3 ? importStep(state) : reviewStep(state);
  const message = state.message
    ? `<p class="onboarding-result" role="status" aria-live="polite">${escapeHtml(state.message)}</p>`
    : `<p class="onboarding-result" role="status" aria-live="polite"></p>`;
  return `<section class="panel onboarding-panel" aria-busy="${state.submitting}">
    <p class="eyebrow">Get started</p><h1>Welcome to CommonKit</h1>
    <p class="setup-intro">Set up this computer in a few guided steps. You will review a preview before CommonKit changes anything.</p>
    ${progress}${content}${message}</section>`;
}

function githubStep({ auth, draft }: OnboardingViewState): string {
  const authBlock = auth.state === "authenticated"
    ? `<div class="auth-card auth-connected"><span class="auth-dot" aria-hidden="true"></span><div><small>GitHub connected</small><p>Signed in as <strong>${escapeHtml(auth.login)}</strong></p></div></div>`
    : `<div class="auth-card"><div><h3>Keep your setup private and portable</h3><p>CommonKit stores configuration—not passwords—in a private repository on GitHub so your computers can stay in sync.</p></div><button type="button" class="primary" id="github-sign-in" ${auth.state === "checking" || auth.state === "authenticating" ? "disabled" : ""}>${auth.state === "authenticating" ? "Opening GitHub…" : "Sign in with GitHub"}</button>${auth.state === "error" ? `<p class="form-error" role="alert">${escapeHtml(auth.message)}</p>` : ""}</div>`;
  if (auth.state !== "authenticated") return `<div class="setup-step"><h2 id="setup-heading" tabindex="-1">Connect GitHub</h2>${authBlock}</div>`;
  return `<form id="onboarding-form" class="setup-step" data-step="1">
    <h2 id="setup-heading" tabindex="-1">Choose your starting point</h2>${authBlock}
    <fieldset class="choice-grid"><legend class="sr-only">Starting point</legend>
      ${choice("mode", "create", "Create a new private setup", "Start fresh and let CommonKit create the private repository.", draft.mode === "create")}
      ${choice("mode", "connect", "Connect an existing setup", "Use a CommonKit repository you already have.", draft.mode === "connect")}
    </fieldset>
    ${draft.mode === "create"
      ? field("repositoryName", "Name your setup", draft.repositoryName, "For example, commonkit. GitHub will keep it private.")
      : field("repository", "GitHub repository", draft.repository, "For example, your-name/commonkit.")}
    <div class="setup-actions"><button class="primary" type="submit">Continue</button></div>
  </form>`;
}

function computerStep({ draft }: OnboardingViewState): string {
  return `<form id="onboarding-form" class="setup-step" data-step="2">
    <h2 id="setup-heading" tabindex="-1">Set up this computer</h2>
    <p>Give this computer a recognizable name and choose where to try CommonKit.</p>
    ${field("computerName", "Computer name", draft.computerName, "For example, al-macbook. You can change the display name later.")}
    ${field("targetRoot", "Folder to manage", draft.targetRoot, "For a safe first test, choose an empty folder. CommonKit previews every change before applying it.", true)}
    <label class="safe-choice"><input type="checkbox" id="safe-test-folder" ${draft.targetRoot.endsWith("/CommonKitManaged") ? "checked" : ""}> Use a safe test folder first</label>
    <details class="advanced"><summary>Advanced local settings</summary>
      ${field("kitDirectory", "Local setup folder", draft.kitDirectory, "Where CommonKit keeps its private Git checkout on this computer.", true)}
      ${field("loadout", "Settings profile ID", draft.loadout, "The portable profile selected for this computer.")}
      ${optionalField("projectLoadout", "Project settings profile", draft.projectLoadout, "Optional project-specific layer applied after your personal profile.")}
      ${optionalField("targetOverride", "Computer-specific override", draft.targetOverride, "Optional final layer for this computer only.")}
    </details>
    ${actions(true, "Continue")}
  </form>`;
}

function importStep({ draft }: OnboardingViewState): string {
  return `<form id="onboarding-form" class="setup-step" data-step="3">
    <h2 id="setup-heading" tabindex="-1">Bring in existing settings</h2>
    <p>Start empty for the first test, or import settings you already manage elsewhere.</p>
    <fieldset class="choice-grid"><legend class="sr-only">Settings source</legend>
      ${choice("provider", "native", "Start with CommonKit", "Begin with an empty, portable setup.", draft.provider === "native")}
      ${choice("provider", "apm", "Import an APM setup", "Use an existing APM manifest and lockfile.", draft.provider === "apm")}
      ${choice("provider", "chezmoi", "Import a chezmoi setup", "Use an existing chezmoi source directory.", draft.provider === "chezmoi")}
    </fieldset>
    ${providerDetails(draft)}
    ${actions(true, "Review setup")}
  </form>`;
}

function reviewStep({ auth, draft, submitting }: OnboardingViewState): string {
  const login = auth.state === "authenticated" ? auth.login : "";
  const repository = draft.mode === "create" ? `${login}/${draft.repositoryName}` : draft.repository;
  const imported = draft.provider === "native" ? "Start with an empty CommonKit setup" : draft.provider === "apm" ? "Import APM settings" : "Import chezmoi settings";
  return `<form id="onboarding-form" class="setup-step" data-step="4">
    <h2 id="setup-heading" tabindex="-1">Review before creating</h2>
    <dl class="review-list"><dt>Private repository</dt><dd>${escapeHtml(repository)}</dd><dt>Local folder</dt><dd>${escapeHtml(draft.kitDirectory)}</dd><dt>Computer</dt><dd>${escapeHtml(draft.computerName)}</dd><dt>Folder to manage</dt><dd>${escapeHtml(draft.targetRoot)}</dd><dt>Personal profile</dt><dd>${escapeHtml(draft.loadout)}</dd>${draft.projectLoadout ? `<dt>Project profile</dt><dd>${escapeHtml(draft.projectLoadout)}</dd>` : ""}${draft.targetOverride ? `<dt>Computer override</dt><dd>${escapeHtml(draft.targetOverride)}</dd>` : ""}<dt>Existing settings</dt><dd>${escapeHtml(imported)}</dd></dl>
    <div class="safety-note"><strong>Nothing on this computer changes yet.</strong><p>CommonKit will prepare a preview for you to review first.</p></div>
    <label class="consent"><input name="publishRegistration" type="checkbox" value="true" ${draft.publishRegistration ? "checked" : ""} required> <span><strong>Save this computer to the private repository</strong><small>This lets your other computers discover it. CommonKit will create and push a registration commit.</small></span></label>
    ${actions(true, submitting ? "Preparing preview…" : "Prepare setup preview", submitting)}
  </form>`;
}

function providerDetails(draft: OnboardingDraft): string {
  if (draft.provider === "native") return `<details class="advanced"><summary>Advanced provider details</summary><p>CommonKit’s built-in provider will create an empty portable setup. No external program is required.</p></details>`;
  if (draft.provider === "apm") return `<details class="advanced"><summary>Advanced provider details</summary>
    ${field("providerExecutable", "APM executable", draft.providerExecutable, "Pinned APM 0.25.0", true)}
    ${field("apmManifest", "Manifest path", draft.apmManifest, "Relative to your setup folder.", true)}
    ${field("apmLockfile", "Lockfile path", draft.apmLockfile, "Relative to your setup folder.", true)}
    ${field("apmPolicy", "Package policy path", draft.apmPolicy, "Relative to your setup folder.", true)}
    <input name="providerVersion" type="hidden" value="0.25.0"></details>`;
  return `<details class="advanced"><summary>Advanced provider details</summary>
    ${field("providerExecutable", "chezmoi executable", draft.providerExecutable, "Pinned chezmoi 2.70.4", true)}
    ${field("chezmoiSource", "Source directory", draft.chezmoiSource, "Relative to your setup folder.", true)}
    ${field("chezmoiConfig", "Config path", draft.chezmoiConfig, "Relative to your setup folder.", true)}
    <input name="providerVersion" type="hidden" value="2.70.4"></details>`;
}

function choice(name: string, value: string, title: string, detail: string, checked: boolean): string {
  return `<label class="choice-card"><input type="radio" name="${name}" value="${value}" ${checked ? "checked" : ""}><span><strong>${title}</strong><small>${detail}</small></span></label>`;
}

function field(name: string, label: string, value: string, help: string, picker = false): string {
  const id = `onboarding-${name}`;
  return `<label class="setup-field" for="${id}"><span>${label}</span><input id="${id}" name="${name}" value="${escapeHtml(value)}" required aria-describedby="${id}-help">${picker ? `<button type="button" class="secondary picker" data-pick${name === "targetRoot" || name === "kitDirectory" ? "-directory" : ""}="${name}" aria-label="Choose ${escapeHtml(label)}">Choose</button>` : ""}<small id="${id}-help">${help}</small></label>`;
}

function optionalField(name: string, label: string, value: string, help: string): string {
  const id = `onboarding-${name}`;
  return `<label class="setup-field" for="${id}"><span>${label} <small>(optional)</small></span><input id="${id}" name="${name}" value="${escapeHtml(value)}" aria-describedby="${id}-help"><small id="${id}-help">${help}</small></label>`;
}

function actions(back: boolean, next: string, disabled = false): string {
  return `<div class="setup-actions">${back ? `<button class="secondary" type="button" id="setup-back">Back</button>` : ""}<button class="primary" type="submit" ${disabled ? "disabled" : ""}>${next}</button></div>`;
}
