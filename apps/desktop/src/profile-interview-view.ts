import { escapeHtml } from "./html.ts";
import { icon } from "./icons.ts";

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
      return `<section>
        <div class="placard"><h1>Revision staged</h1><span>Encrypted</span></div>
        <p class="brief">Only ciphertext was staged for synchronization. CommonKit retained no interview transcript.</p>
      </section>`;
    }
    return `<section>
      <div class="placard"><h1>Encrypt profile</h1><span>Local only</span></div>
      <p class="brief">Your organization cannot recover this profile. Add three public age recipients: this device, your offline recovery key, and the recovery identity held by your password manager.</p>
      <form id="profile-recovery-form">
        <div class="plate-block"><div class="plate-head"><h2>Recovery recipients</h2><span>Public keys</span></div><div class="plate-body">
          <label class="field" for="profile-recipients"><span>Public recovery recipients</span>
            <textarea id="profile-recipients" name="recipients" rows="5" required aria-describedby="profile-recipients-hint">${escapeHtml(state.recipientsText ?? "")}</textarea>
            <small class="hint" id="profile-recipients-hint">One public age1… recipient per line.</small></label>
          <div class="notice caution">${icon.caution()}<p><strong>Never paste a private identity here.</strong> This field takes public recipients only.</p></div>
        </div></div>
        <div class="controls split" style="margin-top:16px">
          <button class="standby" id="profile-cancel" type="button">Discard draft</button>
          <button class="engage" type="submit" ${state.submitting ? "disabled" : ""}>Encrypt locally</button>
        </div>
      </form>
      <p class="status-line" role="status" aria-live="polite">${escapeHtml(state.message)}</p>
    </section>`;
  }

  const question = state.question;
  const source = question.source === "CommonKit" ? "CommonKit core profile" : `Extension from ${question.source}`;
  const value = escapeHtml(state.pendingValue);
  const control = question.valueType === "string_list"
    ? `<textarea id="profile-answer" name="answer" rows="4" aria-describedby="profile-help">${value}</textarea>`
    : `<input id="profile-answer" name="answer" value="${value}" aria-describedby="profile-help">`;
  const hint = question.valueType === "string_list" ? `${escapeHtml(question.help)} One item per line.` : escapeHtml(question.help);
  const confirmation = state.awaitingConfirmation
    ? `<button class="engage" id="profile-confirm" type="button" ${state.submitting ? "disabled" : ""}>Confirm and continue</button>`
    : `<button class="engage" type="submit" ${state.submitting ? "disabled" : ""}>Review answer</button>`;

  return `<section aria-busy="${state.submitting}">
    <div class="placard"><h1>Work profile</h1><span>${state.position} of ${state.total}</span></div>
    <p class="brief">Every question is optional. Nothing is written until you confirm the exact value.</p>
    <form id="profile-interview-form">
      <div class="plate-block"><div class="plate-head"><h2>${escapeHtml(question.label)}</h2><span>${escapeHtml(source)}</span></div><div class="plate-body">
        <label class="field" for="profile-answer"><span class="sr-only">${escapeHtml(question.label)}</span>${control}<small class="hint" id="profile-help">${hint}</small></label>
        ${state.awaitingConfirmation ? `<div class="notice caution">${icon.caution()}<p><strong>Confirm this exact value before it is encrypted.</strong></p></div>` : ""}
      </div></div>
      <div class="controls split" style="margin-top:16px">
        <span class="controls"><button class="standby" id="profile-cancel" type="button">Discard draft</button><button class="standby" id="profile-skip" type="button">Skip</button></span>
        ${confirmation}
      </div>
    </form>
    <p class="status-line" role="status" aria-live="polite">${escapeHtml(state.message)}</p>
  </section>`;
}
