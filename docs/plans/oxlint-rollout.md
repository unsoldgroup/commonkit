# Oxlint capability rollout

Tracking: USG-156

This rollout keeps every change repository-owned and reversible. A stage
advances only when its exit criteria are recorded on USG-156 or a linked
repository issue.

## Stage 1 — CommonKit native pilot

Ship the exact `1.79.0` dependency, native baseline, root config, setup skill,
and compatibility canary in CommonKit. Keep the 27 known warnings advisory and
classify them before enabling `--deny-warnings`.

Exit criteria:

- frozen install, canary, root lint, tests, and typechecks pass;
- macOS arm64 and Linux x64 record separate canary evidence;
- Windows x64 remains undeclared until its native canary evidence exists;
- no GitHub-hosted workflow is added or used as evidence.

## Stage 2 — Baseline distribution proof

Consume `@commonkit/oxlint-config/native` at an exact Git or package revision in
one small Node 24/pnpm repository with no existing JavaScript linter. Use the
setup skill to create a project-owned config and script.

Exit criteria:

- the repository owns its dependency, config, script, and lockfile;
- effective config, included/invalid/ignored fixtures, JSON output, and exit
  codes pass on each platform that repository declares;
- existing typecheck, formatter, and project validation remain separate.

## Stage 3 — ESLint dual-run pilot

Inventory `mail-index` first. If its owner approves the migration, run Oxlint
before ESLint and retain every plugin, processor, rule, or file scope without
proven native parity. If `mail-index` is unsuitable after inventory, select one
of `expedition-insure`, `flipper-unsoldantarctica`, `krill-photo-library`, or
`TREK` and record why.

Exit criteria:

- before/after file coverage and rule dispositions are recorded;
- unsupported behavior stays on ESLint;
- the combined lint command, typecheck, formatter, and project tests pass;
- ESLint removal is a later repository decision backed by rule and ignore
  parity, not a goal of this stage.

## Stage 4 — Portfolio rollout

Migrate the remaining owner-approved ESLint repositories one at a time. Compare
the established Oxlint repositories (`orca` and
`expeditioninsure-os/cloudflare-os`) for version and editor compatibility.
Leave Biome and framework-check repositories unchanged unless their owners
choose a tool migration.

Exit criteria: each repository has its own evidence, exceptions, rollback diff,
and linked issue. No result is inferred from another platform or repository.

## Stage 5 — Optional profiles

Pilot type-aware Oxlint only in a TypeScript 7-compatible repository. Preserve
`tsc` until diagnostic, project-reference, and memory behavior are demonstrated.

Incubate anti-slop separately in one owner-selected repository. Pin commit
`6d538555cb151d4121ed51a27db81890eacf8ae9`, add direct tests for the three
untested generic rules, prove the full Oxlint/`@oxlint/plugins` tuple, enable a
small subset as warnings, and classify every result. Promotion requires focused
tests, recorded provenance, an owner-approved noise threshold, and evidence
from multiple repositories. CommonKit-wide enforcement remains out of scope.
