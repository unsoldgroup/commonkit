export type OnboardingProvider = "native" | "apm" | "chezmoi";

export function onboardingPanel(provider: OnboardingProvider = "native", message = ""): string {
  const apm = provider === "apm" ? `
    <label>APM executable<input name="providerExecutable" required><button type="button" data-pick="providerExecutable">Choose file</button></label>
    <label>Manifest path<input name="apmManifest" value="apm.yml" required><button type="button" data-pick="apmManifest">Choose file</button></label>
    <label>Lockfile path<input name="apmLockfile" value="apm.lock.yaml" required><button type="button" data-pick="apmLockfile">Choose file</button></label>
    <label>Package policy path<input name="apmPolicy" value="apm-policy.yml" required><button type="button" data-pick="apmPolicy">Choose file</button></label>
    <input name="providerVersion" type="hidden" value="0.25.0">` : "";
  const chezmoi = provider === "chezmoi" ? `
    <label>chezmoi executable<input name="providerExecutable" required><button type="button" data-pick="providerExecutable">Choose file</button></label>
    <label>Source directory<input name="chezmoiSource" value="home" required><button type="button" data-pick-directory="chezmoiSource">Choose directory</button></label>
    <label>Config path<input name="chezmoiConfig" value="chezmoi.toml" required><button type="button" data-pick="chezmoiConfig">Choose file</button></label>
    <input name="providerVersion" type="hidden" value="2.70.4">` : "";
  return `<section class="panel onboarding-panel"><p class="eyebrow">Get started</p><h1>Create or connect your kit</h1>
    <p>Provider output is staged and shown as a CommonKit plan before this machine changes.</p>
    <form id="onboarding-form">
      <label>Action<select name="mode"><option value="connect">Connect</option><option value="create">Create private repository</option></select></label>
      <label>GitHub repository<input name="repository" placeholder="owner/commonkit" required></label>
      <label>Kit directory<input name="kitDirectory" required></label>
      <label>Loadout<input name="loadout" value="personal" required></label>
      <label>Target<input name="target" value="workstation" required></label>
      <label>Managed target root<input name="targetRoot" required></label>
      <label>Provider<select id="onboarding-provider" name="provider"><option value="native"${provider === "native" ? " selected" : ""}>Native</option><option value="apm"${provider === "apm" ? " selected" : ""}>APM 0.25.0</option><option value="chezmoi"${provider === "chezmoi" ? " selected" : ""}>chezmoi 2.70.4</option></select></label>
      <label><input name="publishRegistration" type="checkbox" value="true" required> Commit and push this target registration</label>
      ${apm}${chezmoi}<button class="primary" type="submit">Materialize first plan</button>
    </form>${message ? `<p class="onboarding-result">${escapeHtml(message)}</p>` : ""}</section>`;
}

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}
