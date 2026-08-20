---
name: setup-oxlint
description: Use when adding Oxlint to a Node repository, migrating ESLint incrementally, auditing an existing Oxlint setup, or evaluating project-local anti-slop vendoring.
---

# Set up Oxlint

Keep ownership visible: the repository owns its dependency, config, scripts,
lockfile, and exceptions. CommonKit supplies a native baseline and a migration
procedure, not a universal lint command.

## Inspect

Run:

```sh
node <path-to-this-skill>/scripts/inspect-project.mjs --root <repository>
```

Read the repository instructions and every existing lint, format, typecheck,
framework, and custom validation command. Record the current results and file
coverage. Finish when each existing gate has an explicit keep, dual-run, or
parity-proven removal disposition.

## Choose the track

| Inspector result | Action |
| --- | --- |
| `new-oxlint` | Add the native baseline without replacing other gates. |
| `existing-oxlint` | Preserve project semantics; compare the pinned baseline before changing rules. |
| `eslint-dual-run` | Run Oxlint before ESLint. Retain ESLint plugins, processors, and rules without proven native parity. |
| `preserve-biome` | Keep Biome unless the owner explicitly chooses a tool migration. |

Pin `oxlint` exactly to `1.79.0`. Add `@commonkit/oxlint-config` at an exact
reviewed revision. Prefer a root JSON config that extends `native.json`.
Keep `categories`, `env`, `ignorePatterns`, `settings`, `options`, JavaScript
plugins, and type-aware options in the project config; the shared baseline owns
only native rules, native plugins, and overrides. Nested configs need explicit review
because they do not merge automatically.

Add `lint:oxlint` to the project. Compose it before retained ESLint in the
existing `lint` or `check` script. Keep formatters, TypeScript compilation,
CSS/template linters, and project checks separate.

## Verify

Run the baseline canary, the project-owned Oxlint command, retained lint gates,
typechecks, and the normal test suite. Exercise included, invalid, and ignored
fixtures plus JSON output and exit codes. Compare file and rule coverage with
the recorded baseline before removing ESLint behavior. Record native evidence
per supported platform.

## Vendor anti-slop only when selected

Read [anti-slop-vendoring.md](references/anti-slop-vendoring.md) when the owner
selects project-local anti-slop rules. Keep the capability optional, warning-led,
reviewed, and provenance-bound. A complete preset or unattended overwrite fails
the review gate.
