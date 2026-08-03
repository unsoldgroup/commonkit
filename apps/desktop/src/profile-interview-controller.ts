import type { ProfileInterviewQuestion, ProfileInterviewState } from "./profile-interview-view.ts";

const FIELD_IDS = [
  "identity.display_name", "identity.pronouns", "identity.role", "identity.responsibilities",
  "identity.expertise", "communication.tone", "communication.detail_level",
  "communication.preferred_channels", "communication.async_expectations",
  "collaboration.working_style", "collaboration.handoff_preferences",
  "collaboration.meeting_preferences", "collaboration.escalation_preferences",
  "feedback.preferred_style", "feedback.correction_preferences", "feedback.praise_preferences",
  "decisions.decision_style", "decisions.evidence_expectations", "decisions.risk_tolerance",
  "decisions.approval_thresholds", "planning.planning_horizon", "planning.task_breakdown",
  "planning.status_update_preferences", "planning.definition_of_done", "technical.languages",
  "technical.frameworks", "technical.package_managers", "technical.tool_preferences",
  "technical.code_review_preferences", "accessibility.requested_accommodations",
  "accessibility.presentation_preferences", "boundaries.do_not_do", "boundaries.ask_before",
  "boundaries.availability_notes", "agents.response_style", "agents.autonomy_preferences",
  "agents.clarification_preferences", "agents.memory_preferences",
] as const;

const LIST_FIELDS = new Set([
  "identity.responsibilities", "identity.expertise", "communication.preferred_channels",
  "technical.languages", "technical.frameworks", "technical.package_managers",
  "technical.tool_preferences", "accessibility.requested_accommodations",
  "accessibility.presentation_preferences", "boundaries.do_not_do", "boundaries.ask_before",
]);

function sentence(fieldId: string): string {
  const name = fieldId.split(".").at(-1)!.replaceAll("_", " ");
  return name[0].toUpperCase() + name.slice(1);
}

export const coreProfileQuestions: readonly ProfileInterviewQuestion[] = FIELD_IDS.map((fieldId) => ({
  fieldId,
  label: sentence(fieldId),
  help: `Optional: tell agents about your ${sentence(fieldId).toLowerCase()}.`,
  source: "CommonKit",
  valueType: LIST_FIELDS.has(fieldId) ? "string_list" : "string",
}));

export interface ProfileDraftState extends ProfileInterviewState {
  index: number;
  answers: Map<string, string>;
  recipientsText: string;
  encrypted: boolean;
}

export function initialProfileDraft(): ProfileDraftState {
  return {
    index: 0,
    question: coreProfileQuestions[0],
    position: 1,
    total: coreProfileQuestions.length,
    pendingValue: "",
    awaitingConfirmation: false,
    submitting: false,
    message: "Answers stay in memory until you confirm and encrypt them.",
    answers: new Map(),
    recipientsText: "",
    encrypted: false,
  };
}

export function reviewProfileAnswer(state: ProfileDraftState, value: string): void {
  state.pendingValue = value;
  state.awaitingConfirmation = true;
  state.message = "Review the exact value below. It has not been stored yet.";
}

export function advanceProfileQuestion(state: ProfileDraftState, confirmed: boolean): void {
  if (confirmed && state.question && state.pendingValue.trim()) {
    state.answers.set(state.question.fieldId, state.pendingValue);
  }
  state.index += 1;
  state.question = coreProfileQuestions[state.index] ?? null;
  state.position = Math.min(state.index + 1, state.total);
  state.pendingValue = "";
  state.awaitingConfirmation = false;
  state.message = state.question
    ? "Answer, skip, or stop at any time. Unconfirmed text is never persisted."
    : "Interview complete. Add your three public recovery recipients to encrypt locally.";
}

export function cancelProfileDraft(state: ProfileDraftState): void {
  state.answers.clear();
  state.pendingValue = "";
  state.recipientsText = "";
  state.awaitingConfirmation = false;
  state.message = "Draft discarded. No profile values were written.";
}

export function recoveryRecipients(text: string): string[] {
  return [...new Set(text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean))];
}
