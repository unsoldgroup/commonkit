# CommonKit agent instructions

## Validation policy

CommonKit does not use GitHub Actions.

- Do not add, restore, or depend on files under `.github/workflows/`.
- GitHub-hosted jobs, checks, billing, and workflow status must not be used as
  implementation-completion, merge, release, or support gates.
- Validation is local and manually invoked on the relevant macOS, Linux, or
  Windows machine, VM, or explicitly selected non-GitHub runner.
- Record the exact command, platform, artifact, and result. Never infer
  cross-platform support from a different operating system.
- Keep validation and release logic in repository scripts so the same commands
  can run interactively or on a manually invoked platform runner.

## Required local checks

For relevant changes, run:

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
pnpm test
pnpm --dir apps/desktop test
pnpm --dir apps/desktop typecheck
```

Use `scripts/installed-lifecycle.sh` for installed CLI/daemon validation and
the scripts documented in `docs/RELEASING.md` for release artifacts.

## Product boundary

Read `CONTEXT.md`, `docs/adr/`, and `docs/scopes/commonkit-tauri-v1.md` before
changing architecture. Providers compute normalized desired state; CommonKit
alone owns policy validation, plans, target mutation, receipts, verification,
and rollback.
