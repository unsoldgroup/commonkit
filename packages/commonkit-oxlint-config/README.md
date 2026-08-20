# CommonKit Oxlint config

This experimental workspace package contains CommonKit's reviewed native
Oxlint baseline and the routed `setup-oxlint` skill. It is a capability source,
not a lint runner or a project package manager.

Repositories consume `native.json` at an exact reviewed revision. Their root
config remains authoritative for categories, environments, ignores, settings,
options, JavaScript plugins, and type-aware options. Each repository also owns its
Oxlint dependency, scripts, lockfile, exceptions, and any retained ESLint run.

Run the compatibility canary with:

```sh
pnpm --filter @commonkit/oxlint-config test
```

The canary checks the exact Oxlint version, shared config loading, a clean
fixture, an error fixture, an ignored fixture, JSON diagnostics, and command
exit behavior. It emits a compact JSON evidence record.

The shared baseline contains built-in rules and plugins only. The setup skill
documents optional, reviewed anti-slop vendoring; anti-slop is not included or
enabled by this package.
