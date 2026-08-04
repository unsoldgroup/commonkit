import { escapeHtml } from "./html.ts";

export interface ProfileInterviewQuestion {
  fieldId: string;
  label: string;
  help: string;
  source: "CommonKit" | string;
  valueType: "string" | "string_list";
}

export interface ProfileInterviewState {
  question: ProfileInterviewQuestion | null;
  position: number;
  total: number;
  pendingValue: string;
  awaitingConfirmation: boolean;
  submitting: boolean;
  message: string;
  recipientsText?: string;
  encrypted?: boolean;
}

export function profileInterviewPanel(state: ProfileInterviewState): string {
  if (state.question === null) {
    if (state.encrypted) {
      return `<section class="panel profile-interview"><p class="eyebrow">Work profile</p><h1>Encrypted revision staged</h1><p>Only ciphertext was staged for synchronization. CommonKit retained no interview transcript.</p></section>`;
    }
    return `<section class="panel profile-interview"><p class="eyebrow">Work profile</p><h1>Encrypt your confirmed profile</h1><p>Your organization cannot recover this profile. Add three public age recipients: this device, your offline recovery key, and the recovery identity held by your password manager.</p><form id="profile-recovery-form"><label for="profile-recipients"><span>Public recovery recipients</span><textarea id="profile-recipients" name="recipients" rows="5" required>${escapeHtml(state.recipientsText ?? "")}</textarea><small>One public age1… recipient per line. Private identities must never be pasted here.</small></label><div class="setup-actions"><button class="secondary" id="profile-cancel" type="button">Discard draft</button><button class="primary" type="submit" ${state.submitting ? "disabled" : ""}>Encrypt locally</button></div></form><p role="status" aria-live="polite">${escapeHtml(state.message)}</p></section>`;
  }
  const question = state.question;
  const source = question.source === "CommonKit"
    ? "CommonKit core profile"
    : `Extension from ${question.source}`;
  const value = escapeHtml(state.pendingValue);
  const control = question.valueType === "string_list"
    ? `<textarea id="profile-answer" name="answer" rows="4" aria-describedby="profile-help">${value}</textarea><small>Enter one item per line.</small>`
    : `<input id="profile-answer" name="answer" value="${value}" aria-describedby="profile-help">`;
  const confirmation = state.awaitingConfirmation
    ? `<div class="safety-note"><strong>Confirm this exact value before it is encrypted.</strong><p class="profile-confirmation">${value}</p><button class="primary" id="profile-confirm" type="button" ${state.submitting ? "disabled" : ""}>Confirm and continue</button></div>`
    : `<button class="primary" type="submit" ${state.submitting ? "disabled" : ""}>Review answer</button>`;
  return `<section class="panel profile-interview" aria-busy="${state.submitting}">
    <p class="eyebrow">Work profile · ${state.position} of ${state.total}</p>
    <h1>Help agents work well with you</h1>
    <form id="profile-interview-form"><label for="profile-answer"><span>${escapeHtml(question.label)}</span>${control}<small id="profile-help">${escapeHtml(question.help)}</small></label>
    <p class="source-label">${escapeHtml(source)}</p><div class="setup-actions"><button class="secondary" id="profile-cancel" type="button">Discard draft</button><button class="secondary" id="profile-skip" type="button">Skip</button>${confirmation}</div></form>
    <p role="status" aria-live="polite">${escapeHtml(state.message)}</p>
  </section>`;
}
