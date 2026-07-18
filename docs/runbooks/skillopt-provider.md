# SkillOpt provider operator runbook

SkillOpt is installed as an external provider. Do not copy its source or Python
dependencies into CommonKit and do not run its adoption or scheduling paths.

## Activity is not optimization

The Claude plugin's asynchronous `SessionEnd` hook only appends
`<UTC-RFC3339>\t<absolute-project-path>` to
`~/.skillopt-sleep/session-end.log`. `commonkit skills activity` reads matching
markers as a repository-scoped freshness signal; it does not open transcripts,
invoke SkillOpt, call a model, or spend budget. Codex has no equivalent hook.

Harvesting is a separate explicit job. Review proposed examples before import
into `eval/skillopt/<campaign>/{train,validation,test}`. Use `commonkit skills
opportunities` to count reviewed evidence per canonical
`.agents/skills/<name>/SKILL.md`; never route one campaign's failures into a
different or general-purpose skill.

## Install and check the pinned provider

The repository lock is `providers/skillopt/<version>/provider-lock.json`. The
environment named by CommonKit must contain ordinary, non-symlink files at
`bin/python` and `bin/skillopt-sleep`, plus
`commonkit-provider-lock.json`. Installation uses `uv pip compile
--generate-hashes` and `uv pip sync --require-hashes` in a dedicated
environment.

Run:

```text
commonkit skills provider check --environment <provider-env> --lock <provider-lock.json>
```

A passing check means the installed package reports the exact locked version
and the environment marker matches the supported adapter contract. It is a
compatibility check, not a security attestation.

## Run an optimization safely

1. Select one Git-owned instruction-only `SKILL.md` and confirm its digest.
2. Review the evaluation task file. It must use
   `skillopt_sleep.tasks.v1`, set `reviewed: true`, contain at least one task,
   and pass secret scanning.
3. Preview and explicitly approve any redacted evidence. Never stage a raw
   transcript.
4. Select an allowlisted backend and immutable model where available. Supply
   only the credential variables accepted by CommonKit.
5. Run optimization. CommonKit creates and destroys a private staging area and
   imports only strict JSON, `manifest.json`, and `proposed_SKILL.md`.
6. Review the immutable candidate diff, provenance, regressions, and policy
   results. Provider success is not approval.
7. Promote only through the separate digest-bound CommonKit approval flow.

Never invoke `skillopt-sleep` directly for production runs. Never use `adopt`,
`schedule`, `unschedule`, or `--auto-adopt`.

## Upgrade compatibility procedure

1. Obtain the official release version, wheel SHA-256, and source revision.
2. Add a proposed lock and fixtures without changing the active lock.
3. Run `commonkit skills provider upgrade` against a disposable provider root.
4. Inspect the generated hash-locked requirements and confirm the direct
   `skillopt==<version>` requirement carries the expected wheel hash.
5. Require all adapter-contract fixtures to pass: exact version probe, strict
   JSON, staged-file layout, regular-file containment, no adopted paths, and
   byte-identical input skill.
6. Replay the fixed behavioral corpus against both current and proposed
   providers. Review score deltas, rejected edits, privacy behavior, cost, and
   runtime changes; interface compatibility alone is insufficient.
7. Review the immutable upgrade report. Activation requires a separate named
   approval and re-checks the installed environment.
8. Update the repository lock only after approval. Keep the prior environment
   until the new provider has completed canary runs.

If any step fails, classify the release as unsupported, leave the active lock
unchanged, and update the adapter/tests before retrying. Do not repair an
incompatible environment in place.

## Incident and rollback

- Stop new optimization runs; do not delete candidate or upgrade receipts.
- Restore selection of the previously pinned environment and lock.
- Reject candidates produced by the suspect provider version.
- If a candidate was promoted, use the CommonKit promotion receipt and normal
  Git/APM/reconciliation rollback path; never ask SkillOpt to reverse adoption.
- Record provider version, package digest, command-contract version, model,
  harness, configuration, input digests, and affected candidate IDs.

