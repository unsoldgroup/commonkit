# CommonKit repository instructions

## Validation and automation

CommonKit does not use GitHub Actions.

- Do not add or restore `.github/workflows/`.
- Do not wait for, query, rerun, or report GitHub-hosted workflows as evidence.
- GitHub Actions must not be a completion, merge, release, or support gate.
- Tests and release checks are local and manually invoked on the named target
  platform or on an explicitly selected non-GitHub runner.
- Preserve validation logic in scripts committed under `scripts/`, not in a
  provider-specific hosted-workflow format.
- A platform is supported only when its manually recorded native validation
  evidence exists. Do not substitute macOS evidence for Linux or Windows.

## Development

- Node.js 24 or newer; use pnpm.
- Read `CONTEXT.md`, applicable ADRs, and the v1 scope before architectural work.
- Use test-first changes for reconciliation, recovery, credential, provider,
  and target-mutation behavior.
- Preserve the provider/adapter boundary: providers compute desired resources;
  adapters perform authorized target mutation through CommonKit transactions.
- Never commit secret plaintext or put it in plans, receipts, logs, diagnostics,
  fixtures, or portable metadata.

## Verification

Run the relevant local suites directly. The standard complete check is:

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
pnpm test
pnpm --dir apps/desktop test
pnpm --dir apps/desktop typecheck
```

Installed lifecycle and release commands are documented in
`docs/RELEASING.md`. GitHub-hosted status is never required.

## Done status

Al must never have to guess whether the work is finished. This rule applies to
Claude, Codex, and every delegated agent. End every task response with one of
these status lines as the final line:

```
✅ READY TO CLOSE - ALL DONE — <what is finished>
🚧 NOT DONE — <the single next step, and what is blocking it if anything>
```

Use `✅ READY TO CLOSE - ALL DONE` only when the requested changes are finished,
the relevant checks passed, and no follow-up work remains. Otherwise use
`🚧 NOT DONE` and name the single next step or blocker.
