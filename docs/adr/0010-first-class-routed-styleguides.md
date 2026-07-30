# ADR 0010: First-class routed styleguides

Status: Accepted  
Date: 2026-07-30  
Tracking: USG-59

## Context

CommonKit can distribute skills through its version-pinned APM provider, but a
loadout cannot state that one skill is its writing guide. Always-on prose would
charge every turn, make routing less precise, and turn a form guide into a
global voice policy. A public lint command would also make a heuristic scorer
look authoritative.

## Decision

Add an optional, single-valued `capabilities.styleguide` selection:

```yaml
capabilities:
  styleguide:
    skillId: technical-writing
    activation: routed
```

Only `routed` activation is supported. Later layers replace the complete
selection. Organization policy can deny styleguides or pin the selected skill.
CommonKit resolves the selection against exactly one staged APM styleguide
export before planning. Missing, ambiguous, denied, unpinned, or malformed
selections fail closed.

A typed descriptor declares the stable styleguide and exported skill IDs,
modes, prose scopes, exclusions, evaluation suite, upstream revision,
retention map, and APM package provenance. A canonical binding of the
selection and all descriptor, evaluation, manifest, lock, and retention
digests participates in normalized state and provider-input provenance. A
changed binding makes an existing plan stale through the normal plan checks.

The first package is the experimental `commonkit-styleguide` APM package. It
exports `technical-writing` for Claude and Codex. Its `strict` mode covers
procedures, runbooks, safety text, and error messages. Its `technical` mode
covers technical documentation and engineering prose. Source code, identifiers,
commands, quoted source, marketing, essays, creative writing, and
voice-sensitive prose are excluded.

The package adapts the upstream STE writing skill at revision
`b912d5fa59f368253683af2ebfac64ad6d08312d`. It retains the MIT notice,
adaptation notes, and a section-level retention map. A deterministic Node 24
port of the upstream heuristic reports “writing-form violations per 100
words.” It is internal to evaluation and does not add a `commonkit lint`
command or reconciliation gate.

## Consequences

- Existing loadouts remain valid because the capability is optional.
- The guide adds router text only until an agent loads it.
- A loadout cannot blend multiple writing systems in version 1.
- APM continues to own package resolution, compilation, and lock integrity.
- Providers compute staged desired resources. Only CommonKit adapters mutate
  managed targets.
- macOS keeps the external-provider fail-closed boundary. Until a proven
  disposable-user or VM isolation helper exists, a reviewed APM package may
  use the native provider fallback to materialize the exact Git-owned skill
  bytes. The styleguide binding still includes the APM manifest, lock,
  descriptor, evaluation-suite, and retention-map digests.
- SkillOpt may produce isolated candidates, but human review and Git promotion
  remain mandatory.
- Promotion from experimental requires the pinned model thresholds, mandatory
  fidelity and routing assertions, no long-form regression, and native
  evidence for every declared supported platform.
- The experimental version declares macOS arm64 and Linux x64 support.
  Windows is unsupported until a native Windows run records equivalent
  scorer, APM install, frozen replay, audit, and output-integrity evidence.
