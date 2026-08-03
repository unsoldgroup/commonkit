# Dogfood inventory: layers, providers, and coverage

Resolves USG-77 on the wayfinder map USG-65.

This is the concrete inventory behind the day-one dogfood surface named in
USG-67 ("whole development environment, both machines, applying from day one,
credential references but never secret material").

Inventory date: 2026-08-03. Machines: Al's macOS arm64 workstation, and the
Hostinger VPS (`72.60.44.157`, Ubuntu 24.04.4 x86_64, tailnet `100.71.109.66`).

## Headline finding

**The surface named in USG-67 is larger than CommonKit can manage today.** This
is a capability fact read from the code, not a scoping preference. The gap is
concentrated in four places: package and toolchain management, adoption of
pre-existing symlink farms, system-level services, and macOS provider
execution.

Everything in the "Manageable today" section below can be applied with the
adapters that ship. Everything in "Not manageable today" needs either a new
adapter, a change to an existing contract, or a deliberate decision to leave it
outside CommonKit.

## What CommonKit can actually do

Read from `crates/commonkit-adapters` and `crates/commonkit-contracts`.

### Adapters that mutate targets

| Adapter id | `resource_type` | Source |
| --- | --- | --- |
| `files` | `file` | `crates/commonkit-adapters/src/files.rs:148` |
| `ssh-files` | `remote-file` | `crates/commonkit-adapters/src/ssh_files.rs:66` |
| `service` | `service` | `crates/commonkit-adapters/src/service_lifecycle.rs:158` |
| (relay) | `mcp-relay` | `crates/commonkit-relay/src/transaction.rs:228` |

The filesystem vocabulary is closed:
`FilesystemIntent::{File, Directory, Symlink, Remove}`
(`crates/commonkit-adapters/src/resources.rs:168-191`). There is nothing else.

Service management is name-and-invocation only. `ServiceSpec` carries
`{name, executable, arguments, environment, startMode}`
(`service_lifecycle.rs:19-27`). Backends are `launchctl bootstrap/kickstart/bootout
gui/current`, `systemctl --user`, and `schtasks.exe`. **`--user` is hardcoded** —
system-level units are not addressable.

Symlinks are first-class but constrained: `SafeSymlinkTarget` requires the
target to be **relative** and lexically contained within the managed root, and
rejects absolute targets outright (`resources.rs:98-140`). Separately,
CommonKit **refuses to traverse pre-existing symlinks when inspecting a
target** (`crates/commonkit-adapters/tests/target_filesystem.rs:42-62`).

### Providers

Exactly three ship.

| Provider | Computes | External binary | Platforms |
| --- | --- | --- | --- |
| `apm` | agent context into `.claude`/`.codex`/`.agents`, plus MCP declarations cross-checked against a staged `.mcp.json` | APM 0.25.0 | Linux + Windows x86_64. **Fails closed on macOS** |
| `chezmoi` | files, directories, modes, safe relative symlinks, destination-independent templates | chezmoi 2.70.4 | Controller OS/arch must equal target's. **Fails closed on macOS** |
| `native` | the mandated fallback wherever the above fail closed | none | all |

Pins live in `providers/provider-release-lock.json`. `providers/skillopt/0.2.0/`
computes candidates; it is not a target mutator.

**Consequence for this dogfood run:** the Mac can only ever use `native`. The
sandboxed external-provider path is exercised on the VPS alone.

### Credentials

Not an adapter. A reference/readiness/resolver layer with four schemes —
`env://`, `file://`, `keychain://`, `bws://`
(`crates/commonkit-adapters/src/credentials.rs:33-42`). `SecretValue` has no
`Serialize` and no `Clone`, and zeroizes on drop (`:203-223`). This is what
"references, not secret material" means in the data model: CommonKit records
where a secret comes from and where it must land, and never persists the value.

### Layers

`LayerKind` (`crates/commonkit-contracts/src/lib.rs:767`) with wire values
`public_base`, `organization_policy`, `personal_kit`, `project_loadout`,
`target_overrides`; precedence 0-4 at `crates/commonkit-config/src/lib.rs:270-278`.
`public_base` and `organization_policy` are mandatory; each kind appears at
most once.

All five share one closed four-field spec vocabulary (`V1_LAYER_SPEC_FIELDS`):
`securityPolicy`, `contextBudget`, `files`, and `capabilities`.

Merge rules (`v1_merge_rules`): `securityPolicy.deniedPaths` set-union and
`capabilities.styleguide` whole-replace; everything else recursively merges or
replaces according to its JSON type. **`$delete` is registered nowhere** — no
later layer can remove anything an earlier layer established.

Monotonicity is enforced in code, not by convention: `enforce_policy_floor`
(`crates/commonkit-core/src/lib.rs:242-287`) applies five checks to
`SecurityPolicy` — denials must stay a superset, required controls stay `true`,
allowlists may only shrink, minimums only rise, maximums only fall.

Storage: sibling files at `<kit>/layers/<id>.json` in a Git repo, targets at
`<kit>/targets/<id>.json`, each content-addressed excluding its own digest
field.

## Layer assignment

Al is currently both the organization (Unsold Group) and the individual, so the
boundary has to be drawn deliberately rather than discovered. The rule applied
below: **organization policy holds only what must never be weakened on any
machine**, because the floor is monotonic and nothing can later remove it.
Everything Al might legitimately want different on one machine goes in personal
kit or target overrides.

| Layer | Holds |
| --- | --- |
| `public_base` | CommonKit's shipped defaults. Not authored. |
| `organization_policy` | Security floor only: denied paths (`~/.ssh`, `~/.gnupg`, the three `~/.config/gws-*` token caches), required redaction on diagnostics, the credential schemes permitted (`bws://` and `env://`; `file://` denied), maximum provider network posture. Deliberately small — every entry here is permanent. |
| `personal_kit` | Al's cross-machine material: the `~/.agents/` skill and agent corpus, `~/bin` scripts, shell and Git configuration, the `~/.claude` and `~/.codex` fan-out topology, credential *references* for `BWS_ACCESS_TOKEN` and `LINEAR_API_TOKEN`. |
| `project_loadout` | Per-repo agent bundles — the seven repos currently carrying `.claude/skills.manifest` and `agents.manifest`. |
| `target_overrides` | Machine-specific facts: which LaunchAgents exist on the Mac, which `systemctl --user` units exist on the VPS, port bindings, and the Node version each service pins. |

## Manageable today

### Mac

| Item | Path | Layer | Provider | Adapter | Notes |
| --- | --- | --- | --- | --- | --- |
| Skills corpus, active | `~/.agents/skills/` (52 real dirs) | personal_kit | native | `files` | Bulk of the value. |
| Skills library | `~/.agents/skills-library/` (130 real dirs) | personal_kit | native | `files` | |
| Claude agents | `~/.agents/claude-agents/` (14) | personal_kit | native | `files` | |
| Dev-pipeline personas | `~/.agents/agents/` (7) | personal_kit | native | `files` | |
| Claude skill fan-out | `~/.claude/skills/` (86 links) | personal_kit | native | `files` (Symlink) | Requires managed root at `$HOME` and rewriting links to relative. See gap G2. |
| Claude agent fan-out | `~/.claude/agents/` (13 links) | personal_kit | native | `files` (Symlink) | Same. |
| Codex skill fan-out | `~/.codex/skills/` (14 links) | personal_kit | native | `files` (Symlink) | 12 are absolute and must be rewritten relative. See gap G2. |
| `~/bin` scripts | 25 entries | personal_kit | native | `files` | 5 are Mach-O binaries — content-addressed, large but valid. |
| Shell configuration | `~/.zshrc`, `~/.zprofile`, `~/.zshenv` | personal_kit | native | `files` | Managed as files. Secrets must move out first — see finding F1. |
| Git configuration | `~/.gitconfig` | personal_kit | native | `files` | |
| SSH client config | `~/.ssh/config` | — | — | — | **Denied.** `is_forbidden_path` bans `~/.ssh` structurally. |
| Per-repo bundles | 7 repos under `~/code/` | project_loadout | native | `files` | Replaces `sync-repo-skills`. Fixes the dangling-link bug it has today. |
| LaunchAgents | 13 plists, 10 owned, all loaded | target_overrides | native | `service` | `ServiceSpec` covers name/exec/args/env/startMode, which is enough to pin Node per service. No field-level plist ownership. |
| Local MCP stack | mail-index 3765, context-mode 3766, posthog 3767 | target_overrides | native | `service` + `mcp-relay` | |

### VPS

| Item | Path | Layer | Provider | Adapter | Notes |
| --- | --- | --- | --- | --- | --- |
| User services | `/root/.config/systemd/user/` — board-hub, context-mode-server, linear-discord-relay, public-docs, remotion-studio | target_overrides | apm/chezmoi (sandboxed) | `service` | The only place the sandboxed provider path gets exercised. |
| Agent material | `/root/clawd/.claude/` | personal_kit | apm | `ssh-files` | |
| Remote file state | various under `/root/` | project_loadout | chezmoi | `ssh-files` | Controller OS/arch must match target — a macOS controller **cannot** drive the Linux chezmoi provider. Must be driven from a Linux controller or fall back to `native`. |

## Not manageable today

| # | Gap | Blocks | Evidence |
| --- | --- | --- | --- |
| G1 | **Packages and language toolchains.** No apt, brew, nix, winget, rustup, nvm, fnm, pyenv, mise, global npm, or global cargo. | 162 brew formulae, 4 casks; Node via fnm (Mac) and nvm (VPS); Rust via brew; pnpm, bun, deno, and every `~/bin`-adjacent CLI. Toolchains were explicitly named in the USG-67 surface. | No provider or adapter exists. |
| G2 | **Adoption of pre-existing symlink farms.** Absolute symlink targets are rejected, and CommonKit refuses to traverse existing symlinks when inspecting a target. | The entire `~/.claude` / `~/.codex` / `~/.agents` topology as it stands today. Manageable only after links are rewritten relative under a single managed root. | `resources.rs:98-140`; `tests/target_filesystem.rs:42-62`. |
| G3 | **System-level services.** `--user` is hardcoded in the systemd backend. | VPS Caddy, cloudflared, docker, sshd, tailscaled, and the four GitHub Actions runners. | `service_lifecycle.rs:72-122`. |
| G4 | **macOS external provider execution.** Both pinned providers fail closed; there is no macOS asset in the release lock. | The Mac can only use `native`. The sandboxed-provider contract is unprovable on the machine Al uses most. | `providers/provider-release-lock.json`; `docs/SUPPORT-MATRIX.md`. |
| G5 | **Inert layer fields (closed).** `adapters`, `arguments`, `credentials`, `databases`, `denials`, `hooks`, `plugins`, `relay`, `requirements`, `schedules`, `services`, `settings`, `snapshots`, `targets`, and `theme` are rejected as unsupported v1 layer declarations. | These domains remain available through their actual runtime or daemon configuration surfaces; layers no longer silently accept them. | `V1_LAYER_SPEC_FIELDS` contains only the four consumed fields. |
| G6 | **No composition deletion.** `$delete` is registered nowhere, so a later layer can never remove an item an earlier layer established. | Any future need to drop a skill, agent, or service on one machine only. | `v1_merge_rules`, `config/src/lib.rs:371-387`. |
| G7 | **Environment variables and shell env as resources.** Manageable only indirectly, by owning the dotfile that sets them. | Acceptable for now; noted so it is not mistaken for coverage. | No env resource type. |
| G8 | **SSH keys, `known_hosts`, GPG.** Structurally banned. | `~/.ssh/config` and key material stay hand-managed permanently. | `is_forbidden_path`, `core/src/lib.rs:211-240`. |
| G9 | **Cron, systemd timers, launchd calendar intervals.** | Scheduled work outside CommonKit's own drift scheduler. | No adapter. |
| G10 | **File ownership, ACLs, xattrs, hard links, FIFOs, devices.** | Rarely needed here, but means CommonKit cannot fully reproduce an arbitrary tree. | Filesystem vocabulary is closed. |

## Findings outside the map

**F1 — the credential bootstrap is inverted.** `~/.zshrc` (37 lines) holds
`BWS_ACCESS_TOKEN`, `LINEAR_API_KEY`, and `BW_SESSION` in plaintext, and two
`~/bin` scripts carry hard-coded tokens. The token that unlocks the secret store
lives in a dotfile, and `bwu()` rewrites `~/.zshrc` in place. Managing `~/.zshrc`
as a CommonKit file resource would put those values into a content-addressed
plan and a Git-backed kit. **The secrets must move out before that file is
managed.** This is exactly the problem credential references exist to solve.

**F2 — version drift is already live on the Mac.** LaunchAgents pin Node
v24.13.0; the interactive shell resolves v25.5.0. Two Claude hooks and two
`~/bin` shims execute out of `~/code/commonkit`, so relocating that checkout
breaks the environment. Both are the class of drift CommonKit is meant to
surface, and neither is currently visible to anything.

**F3 — the VPS cannot run CommonKit's own test gates.** No Rust toolchain is
installed (`rustc`/`cargo` absent, no `/root/.cargo`). USG-70's Linux
requalification must provision a temporary toolchain, as the earlier `e98dee5`
run did.

**F4 — the OpenClaw stack is retired.** `openclaw-gateway`, `claw-router`,
`claw-dashboard-push`, and `vault-sync` are dead or disabled. They are not part
of the dogfood surface, and repository documentation that describes them as
running is stale.

**F5 — VPS health, unrelated to CommonKit but relevant to applying there.** Swap
is fully exhausted (4.0/4.0 GiB), load is ~12 on 8 cores, five cloudflared
ingress rules point at ports with no listener (3847, 18789, 8788, 3001, 3101),
`obs-review` fails daily at 08:00, and stale `workerd` and node-inspector
sockets hold ports whose owning PIDs are gone. Applying configuration to a host
under this much memory pressure is a poor first test of rollback.

## Recommendation

Split the day-one surface at the capability boundary rather than at a domain
boundary. Apply to everything in "Manageable today", which is the material Al
edits weekly and where the plan/receipt/verify/rollback loop earns its keep.
Leave G1-G10 outside CommonKit for this run and decide separately, per gap,
whether it warrants an adapter or stays permanently out of scope.

Resolving G2 is a precondition, not an option: the symlink farm must be
rewritten to relative links under a single managed root before any of the agent
material can be adopted.
