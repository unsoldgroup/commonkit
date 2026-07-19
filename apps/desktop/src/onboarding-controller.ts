import type { OnboardingDraft } from "./onboarding-view.ts";

type EntrySource = Iterable<[string, FormDataEntryValue | string]>;

const draftKeys = new Set<keyof OnboardingDraft>([
  "mode", "repositoryName", "repository", "kitDirectory", "loadout", "computerName",
  "targetRoot", "provider", "providerExecutable", "providerVersion", "apmManifest",
  "apmLockfile", "apmPolicy", "chezmoiSource", "chezmoiConfig",
]);

export function applyOnboardingValues(draft: OnboardingDraft, values: EntrySource): OnboardingDraft {
  for (const [key, raw] of values) {
    if (!draftKeys.has(key as keyof OnboardingDraft) || typeof raw !== "string") continue;
    (draft as unknown as Record<string, unknown>)[key] = raw;
  }
  return draft;
}

export function onboardingRequest(draft: OnboardingDraft, githubLogin: string): Record<string, unknown> {
  const repository = draft.mode === "create" ? `${githubLogin}/${draft.repositoryName}` : draft.repository;
  const request: Record<string, unknown> = {
    mode: draft.mode,
    repository,
    kitDirectory: draft.kitDirectory,
    loadout: draft.loadout,
    target: draft.computerName,
    targetRoot: draft.targetRoot,
    provider: draft.provider,
    publishRegistration: draft.publishRegistration,
  };
  if (draft.provider === "apm") {
    Object.assign(request, {
      providerExecutable: draft.providerExecutable,
      providerVersion: "0.25.0",
      apmManifest: draft.apmManifest,
      apmLockfile: draft.apmLockfile,
      apmPolicy: draft.apmPolicy,
    });
  } else if (draft.provider === "chezmoi") {
    Object.assign(request, {
      providerExecutable: draft.providerExecutable,
      providerVersion: "2.70.4",
      chezmoiSource: draft.chezmoiSource,
      chezmoiConfig: draft.chezmoiConfig,
    });
  }
  return request;
}
