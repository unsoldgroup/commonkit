# CommonKit

CommonKit is a cross-platform desired-state runtime for complete developer
environments. It composes a public base, a non-overridable organization
security floor, a personal kit, a project loadout, and target overrides; then
it inspects, plans, applies, verifies, and can roll back changes on local and
SSH targets.

CommonKit is not an agent-package manager or a dotfile engine. Version-pinned
providers compute normalized desired resources in isolation. CommonKit alone
owns target policy, mutation, receipts, verification, recovery, services,
credentials, the persistent MCP relay, and mutable-state snapshots.

Durable remote jobs and browser benchmark shards are handled by the Rust
`commonkit-execd` service. Remote Claude/Codex sessions receive repository-declared
tasks, issue context, plans, and skills through CommonKit MCP; execution continues
when the submitting session disconnects. See
[`docs/scopes/durable-execution-v1.md`](docs/scopes/durable-execution-v1.md).

This repository is a pnpm workspace. It also owns the independently
publishable [`mcp-local-relay`](packages/mcp-local-relay) package, which keeps
MCP upstreams warm and exposes them through one persistent local endpoint.
CommonKit remains the desired-state control plane; the relay is its MCP data
plane.

## V1 architecture

- Microsoft APM is the preferred agent-context provider.
- Chezmoi is the preferred home-configuration provider for the supported,
  side-effect-free subset.
- Native providers remain available for migration and fallback.
- On macOS, external providers fail closed unless their isolation contract is
  available; the supported native fallback remains available.
- Provider output is staged in a content-addressed artifact store; providers
  never apply directly to a managed live target.
- Filesystem, service, credential, relay, and snapshot adapters perform the
  approved operations.
- Plans bind desired, observed, policy, provider-input, ownership, and artifact
  digests. A changed input rejects an approved plan as stale.

## Clean-machine source install

Requirements are Rust 1.85+, Node.js 24+, pnpm 10.28.2, Git, and SSH. APM and
chezmoi are optional until a loadout selects them; CommonKit validates their
exact configured versions before use.

```sh
git clone https://github.com/unsoldgroup/commonkit.git
cd commonkit
corepack enable
pnpm install --frozen-lockfile
cargo install --locked --path crates/commonkit-cli
cargo install --locked --path crates/commonkit-service
commonkit --version
commonkitd --port 0
```

For unattended operation, run `commonkit daemon install` and `commonkit daemon start`; see
[the service guide](docs/DAEMON-SERVICE.md). SSH targets use the separately packaged helper in
[the target-helper guide](docs/TARGET-HELPER.md).

### Desktop first run

Open CommonKit from the menu bar and choose **Get started**. The four-step setup connects your
GitHub account, creates or connects a private setup repository, names this computer, and starts
with a safe managed folder. Existing APM or chezmoi settings are optional advanced imports.
CommonKit prepares a preview first; it does not change the selected folder until you review and
explicitly apply that plan.

The development build uses the installed GitHub CLI as its credential broker. If you are already
signed in with `gh`, CommonKit detects that account. Otherwise **Sign in with GitHub** opens the
browser flow and copies its one-time code for you to paste. Tokens remain in GitHub CLI's native
credential storage and are never returned to the desktop webview.

The daemon creates private, platform-native config and state roots plus a
0600/ACL-protected control token. In a second terminal, create or connect a kit:

```sh
commonkit init create \
  --repository unsoldgroup/my-commonkit \
  --kit-directory "$HOME/.config/my-commonkit" \
  --loadout personal \
  --target local \
  --target-root "$HOME"

commonkit status
commonkit sync
commonkit diff <plan-id>
commonkit apply <plan-id> --confirmed
commonkit verify
```

`init connect` uses the same arguments for an existing repository. `sync`
creates a plan; it does not mutate the target. Review `diff` before applying.
Mutating CLI commands require explicit confirmation.

## Transactions and rollback

Every operation has a deterministic identity and runs through
prepare → apply → verify. Receipts transition durably and are hash-chain
validated. Filesystem preimages and provider payloads are content-addressed,
integrity-checked artifacts, so a fresh process can recover without rerunning a
provider, downloading content, resolving templates, or looking up secrets.

```sh
commonkit rollback <run-id> --confirmed
commonkit verify
commonkit diagnostics
```

Rollback executes verified operations in reverse order and restores supported
file bytes, resource types, permissions, symlink targets, and removals. If an
artifact is absent or has the wrong digest, recovery fails closed and preserves
an explicit recoverable receipt instead of guessing. Secret plaintext is never
stored in plans, receipts, logs, diagnostics, or portable artifact metadata.

Snapshot restore similarly stages and authenticates the snapshot and target
preimage, coordinates configured service stop/start commands, and resumes or
rolls back an interrupted transaction on daemon restart.

## Persistent MCP relay

The Rust daemon owns a loopback-only, bearer-authenticated MCP endpoint at
`http://127.0.0.1:3764/mcp`. APM owns portable MCP declarations and generated
client configuration; CommonKit translates those declarations into persistent
relay desired state and applies relay changes transactionally. The legacy Node
`mcp-local-relay` package remains in `packages/mcp-local-relay` during
migration, with shared black-box compatibility fixtures covering initialization,
tool listing and calls, errors, bounded responses, health, and legacy config.

## Development and verification

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
pnpm typecheck
pnpm test
```

CommonKit does not use GitHub Actions. Validation is run locally and manually on
the named macOS, Linux, or Windows machine or an explicitly selected
non-GitHub runner. Signed releases require Apple notarization, Windows signing,
Tauri updater signing, the configured HTTPS update channel, and recorded native
lifecycle evidence; release scripts fail closed when those inputs are absent.

See [CONTEXT.md](CONTEXT.md), [ADR 0005](docs/adr/0005-use-apm-for-agent-context.md),
[the v1 scope](docs/scopes/commonkit-tauri-v1.md), and
[the release policy](docs/RELEASING.md) for the product contract.
