# Oxlint and anti-slop for CommonKit project linting

Date: 2026-08-20

Scope: CommonKit and the local JavaScript/TypeScript project portfolio

Research baseline: Oxlint 1.79.0; anti-slop commit [`6d538555`](https://github.com/dmmulroy/anti-slop/tree/6d538555cb151d4121ed51a27db81890eacf8ae9)

## Decision

Adopt Oxlint as an optional, project-owned lint baseline that CommonKit can distribute and explain. Do not add a universal `commonkit lint` command. Each repository should own its Oxlint dependency, configuration, script, and lockfile. CommonKit should carry a pinned setup or migration skill, a native-rule baseline, a canary, and policy metadata.

Start with native Oxlint rules. Run Oxlint before an existing ESLint command and migrate one repository at a time. Keep ESLint for rules or processors that Oxlint cannot reproduce. Keep formatters, CSS linters, framework template checks, custom repository checks, and TypeScript compilation as separate gates.

Do not adopt anti-slop as a CommonKit-wide required ruleset. It is a young, opinionated Oxlint JavaScript plugin built on an API that Oxlint still excludes from semver. Its full preset reports 574 anti-slop errors in the CommonKit repository. Most findings come from rules that conflict with CommonKit's deliberate boundary-validation code. Incubate a reviewed, pinned subset only after CommonKit adds missing rule tests, measures findings in several repositories, and defines explicit exceptions.

## Why Oxlint fits

Oxlint is a dedicated JavaScript and TypeScript linter built on Oxc. Version 1.79.0 was released on 2026-08-18. The current npm package supports Node `^20.19.0 || >=22.12.0`, including CommonKit's Node 24 minimum, and publishes native bindings for CommonKit's macOS arm64, Linux x64, and Windows x64 targets. See the [Oxlint overview](https://oxc.rs/docs/guide/usage/linter), [1.79.0 release](https://github.com/oxc-project/oxc/releases/tag/oxlint_v1.79.0), and [package manifest](https://github.com/oxc-project/oxc/blob/oxlint_v1.79.0/npm/oxlint/package.json).

Oxlint supplies more than 840 native rules across ESLint core, TypeScript, React, Jest, Vitest, Import, Unicorn, and jsx-a11y. It supports safe fixes, ignore files, nested configuration, multi-file analysis, machine-readable output, and an editor language server. Its default category emphasizes correctness and produces warnings, which supports a low-friction first run. The [quickstart](https://oxc.rs/docs/guide/usage/linter/quickstart) documents local dependencies, package scripts, pre-commit use, output formats, and config inspection.

The core CLI and JSON configuration now follow semver. Oxlint does not treat new findings or corrected ESLint-compatible rule behavior as breaking changes. CommonKit should therefore pin an exact Oxlint version and review upgrades even within a major version. See the [versioning policy](https://oxc.rs/docs/guide/usage/linter/versioning).

### What Oxlint does not replace

Oxlint only covers JavaScript, TypeScript, JSX, TSX, and the script portions of selected component formats. It does not lint CSS, SCSS, HTML, JSON, YAML, Markdown, SQL, shell, or Rust. Vue, Svelte, Angular, Ember, Nuxt, Astro, SvelteKit, and Analog template linting is not available. See the [compatibility matrix](https://oxc.rs/compatibility).

For CommonKit, this means:

- `cargo clippy` remains the Rust linter. Rust is the largest source language in this repository.
- `tsc --noEmit` remains the TypeScript compiler gate during initial adoption.
- Project-specific scripts remain separate. Examples include schema parity, localization, accessibility contrast, and bounded-collection checks.
- Stylelint, Astro checks, and existing formatters remain in place.
- The routed technical-writing styleguide remains a prose capability. It is explicitly not a public lint gate in [ADR 0010](../../adr/0010-first-class-routed-styleguides.md) and the [styleguide README](../../../packages/commonkit-styleguide/README.md).

## Fit with CommonKit's architecture

CommonKit is a desired-state runtime, not a project package manager. Its current [package manager contract](../../../crates/commonkit-contracts/src/lib.rs) handles system and runtime managers, not pnpm project dependencies. The existing Probity boundary is the right precedent: CommonKit can carry policy, hook declarations, and a canary while the package stays in the repository's `package.json` and lockfile. See the [architecture summary](../../../README.md), [provider/adapter ADR](../../adr/0006-providers-compute-adapters-mutate.md), and [bootstrap inventory](../../BOOTSTRAP.md).

The recommended CommonKit integration has four parts:

1. A routed `setup-oxlint` or `migrate-oxlint` skill inspects the repository, proposes a diff, and runs repository-native checks.
2. A small `@commonkit/oxlint-config` package exports native rules, plugins, and overrides. Repositories import it from `oxlint.config.ts` and add project-specific settings.
3. A loadout records the chosen baseline revision, intended project scope, and validation command. It does not run lint on every agent turn.
4. A canary verifies the exact Oxlint version, effective configuration, representative positive and negative fixtures, and command exit behavior.

Oxlint supports config objects imported from shared packages through `oxlint.config.ts`. However, shared configs can extend only `rules`, `plugins`, and `overrides`. Projects must still own ignores, environments, settings, linter options, and JavaScript-plugin declarations. Nested configs do not merge with parent configs automatically. Root-only type-aware options add another monorepo constraint. See [configuration](https://oxc.rs/docs/guide/usage/linter/config) and [nested configuration](https://oxc.rs/docs/guide/usage/linter/nested-config).

A `commonkit lint` wrapper would hide project ownership, complicate nested configs, and make CommonKit responsible for unrelated formatters and framework checks. It would also conflict with the repository's rule that validation logic belongs in committed scripts. The integration should add or preserve project scripts such as `lint:oxlint` and compose them into each repository's existing `lint` or `check` command.

## Local project compatibility

A scan of local package manifests on 2026-08-20 found three useful adoption groups.

| Group | Local examples | Assessment |
| --- | --- | --- |
| Oxlint already used | `orca`, `expeditioninsure-os/cloudflare-os` | Use as reference implementations. Orca already separates native and type-aware Oxlint runs. |
| ESLint used | `expedition-insure`, `flipper-unsoldantarctica`, `krill-photo-library`, `mail-index`, `TREK` | Good incremental migration candidates. Preserve unsupported plugins and custom checks. |
| Biome or framework checks used | `social-pipeline`, `whale-price-tracker`, Astro applications | Do not migrate only for uniformity. Oxlint does not replace formatting or template checks. |

CommonKit itself has no JavaScript linter dependency or lint script. It contains 123 tracked `.ts` files and 33 tracked `.mjs` files, alongside 221 tracked `.rs` files. Oxlint can close a real gap, but only for part of the repository.

The ESLint repositories span legacy ESLint 8, flat-config ESLint 9, and ESLint 10. Oxlint's migrator accepts ESLint 9 and 10 flat configs. Legacy ESLint 8 configs may need the official ESLint migrator first. Local custom plugins require manual registration. Oxlint recommends a dual-run migration with `eslint-plugin-oxlint` disabling overlapping ESLint rules. See [Oxlint's migration guide](https://oxc.rs/docs/guide/usage/linter/migrate-from-eslint) and [ESLint's configuration migration guide](https://eslint.org/docs/latest/use/configure/migration-guide).

Likely repository-specific gaps include:

- `eslint-plugin-unused-imports`, React Refresh, and other non-native plugins may need Oxlint's JavaScript-plugin path or a retained ESLint run.
- Prettier integrations should become a separate formatter command rather than a JavaScript lint plugin.
- Stylelint and contrast checks in Krill remain separate.
- Next.js and React source have broad support, but migration still needs rule-by-rule comparison.
- Astro script blocks can be linted, but Astro templates still need `astro check` or another template-aware tool.

## Type-aware linting

Oxlint delegates semantic rules to `oxlint-tsgolint`, which uses TypeScript Go and targets TypeScript 7. The backend implements 59 of 61 targeted `typescript-eslint` type-aware rules. Oxlint can also emit TypeScript diagnostics with `--type-aware --type-check`. See the [type-aware guide](https://oxc.rs/docs/guide/usage/linter/type-aware) and [tsgolint README](https://github.com/oxc-project/tsgolint/blob/main/README.md).

Type-aware linting should be a separate opt-in profile, not the initial CommonKit default:

- It requires another native package.
- It requires TypeScript 7 behavior and rejects some legacy compiler options, including `baseUrl`.
- Monorepos may need dependent packages built before linting.
- The Oxlint versioning policy still excludes type-aware behavior from semver.
- Oxlint documents incomplete coverage and possible high memory use on very large repositories.

Keep `tsc --noEmit` until a repository proves equivalent diagnostics, project-reference behavior, and memory use. Orca's separate native and type-aware scripts are a safer model than combining both from day one.

## Anti-slop assessment

### Architecture

Anti-slop is a project-local Oxlint JavaScript plugin. Its generic entry point exports 15 syntax and lexical rules through `eslintCompatPlugin`; an optional Effect entry point exports one architecture rule. The rules use Oxlint's ESTree API and do not add another production parser. Most use Oxlint's optimized `createOnce` visitor API. See the [generic entry point](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/src/index.ts), [Effect entry point](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/src/effect/index.ts), and [repository guidance](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/AGENTS.md).

The author intends teams to vendor and modify the source rather than install a fixed npm package. The repository ships an agent skill with a copy script. The copy script refuses to overwrite an existing destination unless the caller passes `--force`. It does not edit the Oxlint config or package manifest itself; the agent does those steps. See the [README](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/README.md), [install skill](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/skills/install-anti-slop/SKILL.md), and [copy script](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/skills/install-anti-slop/scripts/install.mjs).

This architecture complements Oxlint. Oxlint supplies broad correctness and ecosystem rules. Anti-slop encodes narrower opinions about preserving type evidence, avoiding dynamic reflection, parsing boundaries, test seams, and naming. Several rules occupy similar territory to type-aware TypeScript rules, but anti-slop uses syntax, lexical alias resolution, and scope analysis rather than TypeScript's full type system.

Anti-slop does not overlap with CommonKit's technical-writing styleguide. One checks source-code patterns. The other routes a prose-writing skill and keeps its heuristic scorer internal.

### Maturity and maintenance

At the examined commit, anti-slop has an MIT-licensed, private package manifest at version 0.1.0. It has no release tags. One author produced 13 commits between 2026-08-12 and 2026-08-18. The repository is active, but the short history and single-maintainer shape make it an incubation project rather than a stable organization policy. See the [package manifest](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/package.json) and [commit history](https://github.com/dmmulroy/anti-slop/commits/6d538555cb151d4121ed51a27db81890eacf8ae9/).

The pinned upstream `pnpm check` passed locally on macOS. That command ran Oxlint, the declared rule tests, TypeScript 7.0.2, and a check that the install skill's copied assets match `src/`. However, three of the 15 generic rules have no direct test file: `no-chained-type-assertions`, `no-shape-in-symbol-names`, and `no-unknown-parameters`. CommonKit should not make those rules blocking until it adds focused valid and invalid cases.

Upstream CI is one Ubuntu job that installs with a frozen pnpm lockfile on Node 24 and runs `pnpm check`. The command is portable to CommonKit's named local runners, even though CommonKit must not import the GitHub workflow. The vendored skill assets contain production rule source but not the rule tests. CommonKit therefore needs a canonical plugin canary instead of assuming each target repository can rerun upstream compatibility tests. See the [upstream workflow](https://github.com/dmmulroy/anti-slop/blob/6d538555cb151d4121ed51a27db81890eacf8ae9/.github/workflows/ci.yml) and [skill assets](https://github.com/dmmulroy/anti-slop/tree/6d538555cb151d4121ed51a27db81890eacf8ae9/skills/install-anti-slop/assets/anti-slop).

### Noise and false-positive risk

Running every generic anti-slop rule as an error against CommonKit with the upstream pinned Oxlint 1.78.0 produced 574 anti-slop diagnostics:

| Rule | Findings | Assessment |
| --- | ---: | --- |
| `no-runtime-typeof` | 194 | Conflicts with CommonKit's explicit parsers, redaction, config validation, and untrusted payload normalization. High false-positive or policy-conflict risk. |
| `require-safety-comment-for-type-assertion` | 172 | Creates a large comment burden and can reward boilerplate without proving the invariant. High rollout cost. |
| `no-unknown-parameters` | 90 | Rejects the natural input type for boundary parsers. Its only built-in exception is a parameter named `cause`. High policy-conflict risk. |
| `no-unsafe-dictionary-type` | 54 | Rejects `Record<string, unknown>`, which CommonKit uses for untrusted JSON envelopes before validation. High policy-conflict risk. |
| `no-conditional-empty-object-spread` | 23 | Enforces a construction style more than a correctness property. Medium noise risk. |
| `no-known-value-widening` | 14 | Can catch discarded key evidence, but deliberate abstraction to a registry interface is valid. Review manually. |
| `no-shape-in-symbol-names` | 12 | Rejects legitimate domain terms such as `statusShape`. High vocabulary false-positive risk. |
| `no-chained-type-assertions` | 10 | Likely useful, but it lacks a direct upstream test file. Candidate after tests. |
| `no-unknown-returns` | 5 | Useful at domain APIs, but dynamic adapters may intentionally expose unparsed values. Scope narrowly. |

The four broad boundary and assertion rules account for 510 of 574 findings. CommonKit should not “fix” these findings mechanically. A schema library may remove some patterns, but handwritten validation remains legitimate at I/O and adapter boundaries.

Lower-noise candidates for a later advisory pilot are:

- `no-widen-then-assert`, which has focused tests and targets a specific evidence-loss flow.
- `no-chained-type-assertions`, after CommonKit adds direct tests.
- `no-module-mocking`, only in repositories that explicitly require dependency seams.
- `no-reflect-get` and `no-reflect-apply`, only where dynamic object access is not part of the domain.

### Install, configuration, and supply chain

Anti-slop is easy to inspect because the project owns the copied source. It is harder to update because every repository owns a fork. CommonKit would need a provenance record, an upstream commit pin, a retention map, and a reviewed update process similar to the styleguide package.

The upstream install skill is not deterministic enough for CommonKit reconciliation. It tells the agent to query current Oxlint and `@oxlint/plugins` versions during installation. That makes two installations at different times produce different lockfile changes. CommonKit should replace that behavior with a reviewed version tuple.

The install script accepts a caller-provided destination and offers a force-overwrite mode. CommonKit should stage and diff the exact files, reject an existing destination by default, and apply through normal project review. It should not run `npx skills add` or the upstream force path as part of unattended reconciliation.

Oxlint's native-rule path has a smaller execution surface than ESLint. That advantage narrows when a project uses `oxlint.config.ts` and JavaScript plugins: the Node runtime executes the config and imported plugin code. Oxlint labels JavaScript plugins alpha, excludes them from semver, does not support type-aware JavaScript plugin rules, and does not support plugin-provided custom parsers or file formats. See the [JavaScript plugin guide](https://oxc.rs/docs/guide/usage/linter/js-plugins) and [versioning policy](https://oxc.rs/docs/guide/usage/linter/versioning).

For the native CommonKit baseline, prefer a JSON config and built-in plugins. For anti-slop, pin and digest the vendored source plus `oxlint` and `@oxlint/plugins`; review every update; do not enable automatic fixes; and run the plugin in the repository's normal unprivileged development environment.

## Rollout plan

### Stage 1: Native Oxlint pilot in CommonKit

1. Pin the current reviewed Oxlint version in CommonKit's root `devDependencies`.
2. Add a JSON config with native plugins and correctness-focused rules only.
3. Add `lint:oxlint` and compose it into the documented local validation command.
4. Record macOS arm64 and Linux x64 evidence separately. Add Windows evidence before declaring Windows support.
5. Keep `cargo clippy`, TypeScript typechecks, tests, and project-specific checks unchanged.

Oxlint 1.78.0's default config reported 27 warnings across six rules in the current CommonKit tree and exited successfully. That is a manageable baseline. Review the findings, set intentional severities, then use `--deny-warnings` only after the baseline is clean.

### Stage 2: Package the reusable baseline

1. Export only native rules, native plugins, and overrides from `@commonkit/oxlint-config`.
2. Add a setup skill that preserves repository config, scripts, package manager, and lockfile.
3. Add canary fixtures for config loading, rule severity, ignores, output format, and exit codes.
4. Keep project-owned settings, environments, ignores, and root type-aware options in each repository.

### Stage 3: Migrate existing ESLint repositories

1. Run `@oxlint/migrate` against each flat config and capture unsupported details.
2. Run Oxlint before ESLint.
3. Use `eslint-plugin-oxlint` to disable overlap.
4. Retain ESLint for missing plugins, processors, or project rules.
5. Remove ESLint only after rule, ignore, editor, and exit-code parity is proven.

Do not migrate Biome projects unless their owners want a tool change. Uniformity alone does not justify replacing a working combined formatter and linter.

### Stage 4: Type-aware profile

Pilot `oxlint-tsgolint` in a TypeScript 7-compatible repository. Compare results with `tsc` and `typescript-eslint`, measure memory, and preserve `tsc` until parity is demonstrated.

### Stage 5: Anti-slop incubation

1. Pin commit `6d538555` and preserve the MIT license and provenance.
2. Add direct tests for the three untested generic rules.
3. Build a CommonKit fixture corpus with known positive and negative cases.
4. Start a small subset at warning severity in one repository.
5. Classify every finding as defect, useful policy, acceptable exception, or false positive.
6. Promote a rule to blocking only when its false-positive rate and exceptions are documented.

## Promotion criteria

Promote native Oxlint to a standard CommonKit project capability when:

- the exact version and config revision are pinned;
- native checks pass on every declared platform;
- the repository owns its dependency and lockfile;
- the effective config and exit behavior have canary coverage;
- formatters, typechecks, and non-JavaScript linters remain explicit;
- ESLint removal, when applicable, has rule-by-rule parity evidence.

Promote any anti-slop rule only when:

- the rule has focused valid and invalid tests;
- its upstream and CommonKit provenance are recorded;
- a multi-repository corpus shows acceptable noise;
- project owners can opt in or override it without weakening unrelated correctness rules;
- the Oxlint JavaScript-plugin API is either stable or CommonKit pins and tests the full compatible version tuple.

## Sources

Primary external sources:

- [Oxlint overview](https://oxc.rs/docs/guide/usage/linter)
- [Oxlint quickstart](https://oxc.rs/docs/guide/usage/linter/quickstart)
- [Oxlint configuration](https://oxc.rs/docs/guide/usage/linter/config)
- [Oxlint nested configs](https://oxc.rs/docs/guide/usage/linter/nested-config)
- [Oxlint migration from ESLint](https://oxc.rs/docs/guide/usage/linter/migrate-from-eslint)
- [Oxlint JavaScript plugins](https://oxc.rs/docs/guide/usage/linter/js-plugins)
- [Oxlint type-aware linting](https://oxc.rs/docs/guide/usage/linter/type-aware)
- [Oxlint versioning](https://oxc.rs/docs/guide/usage/linter/versioning)
- [Oxlint compatibility matrix](https://oxc.rs/compatibility)
- [Oxlint 1.79.0 release](https://github.com/oxc-project/oxc/releases/tag/oxlint_v1.79.0)
- [tsgolint repository](https://github.com/oxc-project/tsgolint)
- [ESLint configuration migration guide](https://eslint.org/docs/latest/use/configure/migration-guide)
- [anti-slop repository at the examined commit](https://github.com/dmmulroy/anti-slop/tree/6d538555cb151d4121ed51a27db81890eacf8ae9)

Local evidence:

- CommonKit repository scan at the current worktree on 2026-08-20.
- Oxlint 1.78.0 default run against the CommonKit tree: 27 warnings, exit 0.
- Anti-slop 1.78.0 all-rules run against the CommonKit tree: 574 anti-slop errors.
- Anti-slop upstream `pnpm check` at commit `6d538555`: passed on macOS after dependency installation.
