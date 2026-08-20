# Repository-owned Oxlint

Status: experimental native baseline

Tracking: USG-156

CommonKit distributes an Oxlint capability while each repository retains lint
ownership. There is no `commonkit lint` wrapper and no always-on lint policy.

## Ownership contract

| CommonKit owns | Each repository owns |
| --- | --- |
| Reviewed native rules, native plugins, and overrides | Exact Oxlint dependency and lockfile |
| `setup-oxlint` inspection and migration guidance | Root config, categories, environments, ignores, settings, options, and exceptions |
| Compatibility fixtures and evidence shape | `lint:oxlint` and composition with existing validation |
| Anti-slop review and provenance template | Any selected vendored plugin source and JavaScript-plugin declaration |

The baseline is
[`packages/commonkit-oxlint-config/native.json`](../../packages/commonkit-oxlint-config/native.json).
CommonKit's root [`.oxlintrc.json`](../../.oxlintrc.json) demonstrates the
project-owned layer. Nested configs require explicit review because Oxlint does
not merge them automatically.

## Local commands

```sh
pnpm run lint:oxlint
pnpm run test:oxlint-canary
```

Oxlint `1.79.0` is pinned exactly. The initial native correctness baseline
reports 27 non-blocking warnings across 161 files: 12 unnecessary escapes,
five unused variables, five string-prefix suggestions, three unnecessary
spread fallbacks, one control-regex warning, and one unnecessary spread.
`no-debugger` is an error. Warning cleanup is reviewed separately; the command
does not use `--deny-warnings` until the baseline is clean.

Oxlint complements rather than replaces `cargo clippy`, TypeScript compilation,
formatters, CSS and template linters, framework checks, or repository-specific
validation. Type-aware Oxlint remains a separate opt-in experiment because its
backend and behavior are outside the initial native baseline.

## Anti-slop boundary

Anti-slop is not a dependency, preset, or mandatory CommonKit policy. A
repository may vendor selected rule source only through the
[`setup-oxlint` skill](../../packages/commonkit-oxlint-config/skills/setup-oxlint/SKILL.md).
That path pins an upstream commit and compatible tool versions, preserves the
MIT license and copyright notice with its own SHA-256 digest, records per-file
SHA-256 digests, rejects unattended overwrite, adds missing rule tests, starts
selected rules as warnings, and classifies each finding before promotion.

Validation remains local or runs on explicitly selected non-GitHub runners.
CommonKit does not use GitHub Actions.
