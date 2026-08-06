# Kit onboarding handover

Written 2026-08-06. Hand this to Codex (or any agent on any device) to finish
onboarding the CommonKit kit as a plugin.

## What the kit is

`al-unsoldgroup/commonkit` (the kit repo, checked out locally at
`~/.commonkit-kit`) is now a **plugin marketplace**. Its payload:

| Item | Count | Path in repo |
|---|---|---|
| Skills | 58 | `plugin/skills/` |
| Agents | 14 | `plugin/agents/` |
| MCP servers | 23 | `plugin/.mcp.json` |

Six of those servers are stdio processes driven by the `${REFERENCES}` below.
The other seventeen are hosted connectors declared as `http`/`sse` URLs and
authenticated per device over OAuth, so the kit carries no credential for
them at all. On a machine whose Claude account already has the same connector,
Claude Code hides the account copy **once the kit's copy is connected** — an
unauthenticated kit server does not displace anything.

### Required companion plugins

`context-mode` is deliberately **not** carried in `.mcp.json`. Its only
entrypoint is a versioned path inside its own plugin cache
(`…/context-mode/<version>/start.mjs`) with no global binary and no npm
package, so an inlined command would break on its next release. Install it
alongside the kit:

```sh
claude plugin marketplace add context-mode && claude plugin install context-mode@context-mode
```

Marketplace manifest: `.claude-plugin/marketplace.json` (repo root).
Plugin manifests: `plugin/.claude-plugin/plugin.json` and
`plugin/.codex-plugin/plugin.json` (`commonkit-kit`, v0.1.1).

Current head: `53194c6` "Ship the kit as a Claude Code plugin marketplace".

## Why a plugin, not the native provider

The native provider path maps one kit file to one managed path, so this kit
needs ~996 receipted file operations per device. That path is blocked by
USG-139: every receipt entry embeds the full `operationProgress` and
`transitions` arrays, so entry size grows quadratically (1,342 B → 384,693 B
by entry 925). One apply wrote 170 MB and stalled at 93 % of prepare with
nothing materialized.

The plugin path is a single git clone. It sidesteps USG-139 entirely, and it
also carries MCP declarations, which the native provider has no mechanism for.

**Both paths stay.** The plugin is how the kit reaches agents today; the
native provider remains the right answer for files that are not skills,
agents, or MCP config, and it is what CommonKit's receipts and rollback are
built around.

## Verified state on this Mac (Claude Code)

```
claude plugin marketplace add al-unsoldgroup/commonkit
claude plugin install commonkit-kit@commonkit
```

Both succeeded. Confirmed after restart:

- `~/.claude/plugins/installed_plugins.json` → `commonkit-kit@commonkit`,
  scope `user`, `gitCommitSha 53194c6c…`
- Cache at `~/.claude/plugins/cache/commonkit/commonkit-kit/0.1.0/`
  contains 58 skills, 14 agents, `.mcp.json`
- Skills load in-session as `commonkit-kit:<name>`
  (e.g. `commonkit-kit:ast-grep`, `commonkit-kit:ponytail`)
- Agents load as `commonkit-kit:<name>`
  (e.g. `commonkit-kit:linear-ops`, `commonkit-kit:bws-secrets`)
- MCP: `stagehand` connected as
  `mcp__plugin_commonkit-kit_stagehand__*`

Note the cache flattens `plugin/` into the version directory — the installed
tree has `skills/` and `agents/` at its root, not under `plugin/`.

## Do this on Codex

Codex CLI 0.146.0 supports the same marketplace format:

```
codex plugin marketplace add al-unsoldgroup/commonkit
codex plugin add commonkit-kit@commonkit
codex plugin list
```

Differences from Claude Code to expect:

- Codex records plugin state in `~/.codex/config.toml` under
  `[plugins."commonkit-kit@commonkit"]`, not in JSON files. The JSON files in
  `~/.codex/plugins/` are not valid JSON and are not the source of truth.
- Cache lands under `~/.codex/plugins/cache/commonkit/commonkit-kit/0.1.0/`.
- Codex currently carries only 15 skills in `~/.codex/skills/` (a deliberate
  curated subset — see the skills-budget policy in `~/.claude/CLAUDE.md`).
  Installing the plugin adds all 58. **If that floods Codex's skill budget,
  that is the expected tradeoff to evaluate, not a bug.** Decide whether to
  keep the plugin skills enabled on Codex or prune the plugin to a Codex
  profile before assuming either.
- Codex disables plugin MCP servers by default
  (`[plugins."x@y".mcp_servers.z] enabled = false`). Enable per server, do
  not blanket-enable.

## Migration to plugin-only (done on this Mac 2026-08-06)

The kit is now the sole source of skills, agents, and MCP servers. What changed:

| Surface | Before | After |
|---|---|---|
| `~/.claude/skills` | 91 symlinks | 37 (36 `skills-library` + `deckadence`) |
| `~/.claude/agents` | 13 symlinks | 0 |
| `~/.claude.json` `mcpServers` | 8 servers with literal secrets | empty |

The 54 skill and 13 agent symlinks removed were exact plugin duplicates. The
37 that remain are **not** duplicates: 36 are `skills-library` (on-demand
tier, deliberately outside the kit) and `deckadence` is hand-authored with no
sentinel. Removing those would cut capability, not copies.

Undo script for the symlink removal is written to the session scratchpad, and
`~/.claude.json` was backed up alongside it before the MCP entries were cut.

## Required per-device environment

The kit carries **no secret values**. Six references must resolve in the
environment before their servers will start:

```
AHREFS_API_KEY
TWENTY_AUTHORIZATION
STAGEHAND_MODEL_API_KEY
STAGEHAND_BROWSERBASE_API_KEY
STAGEHAND_BROWSERBASE_PROJECT_ID
STAGEHAND_OPENAI_BASE_URL
```

All six live in Bitwarden Secrets Manager. They are resolved at login by
`~/.config/commonkit/kit-env.sh`, sourced from `~/.zprofile`. That script
stores **key names only** — no secret value is written to disk by it.

The bws key for each reference:

| Reference | bws key | Note |
|---|---|---|
| `AHREFS_API_KEY` | `expedition-insure/AHREFS_API_TOKEN` | |
| `TWENTY_AUTHORIZATION` | `twenty/API_KEY_SELFHOST` | kit wants the whole header; bws stores a bare token, so the script prefixes `Bearer ` |
| `STAGEHAND_MODEL_API_KEY` | `OPENAI_API_KEY` | shared project |
| `STAGEHAND_BROWSERBASE_API_KEY` | `stagehand/BROWSERBASE_API_KEY` | an identical copy exists under `expedition-insure/`; prefer the purpose-named one |
| `STAGEHAND_BROWSERBASE_PROJECT_ID` | `stagehand/BROWSERBASE_PROJECT_ID` | same |
| `STAGEHAND_OPENAI_BASE_URL` | composed from `CLOUDFLARE_ACCOUNT_ID` | `https://gateway.ai.cloudflare.com/v1/<account>/ocean/compat` — not stored, built at login |

To set this up on a new device, copy `kit-env.sh` and source it from
`~/.zshrc`:

```sh
[ -n "$AHREFS_API_KEY" ] || { [ -f ~/.config/commonkit/kit-env.sh ] && . ~/.config/commonkit/kit-env.sh; }
```

**It must come after `BWS_ACCESS_TOKEN` is exported.** This bit once: the line
was originally in `~/.zprofile`, but zsh sources `.zprofile` *before*
`.zshrc`, where the token is defined, so `kit-env.sh` always saw an unset
token and silently exported nothing. The guard on `$AHREFS_API_KEY` means
only the outermost shell pays the bws call; nested shells inherit.

Verify with a shell that inherits nothing:

```sh
env -i HOME="$HOME" TERM=xterm /bin/zsh -lic 'printenv AHREFS_API_KEY'
```

A plain `zsh -l` inherits the caller's environment and will pass even when the
config is wrong.

**Terminal-launched agents only.** Agents launched from a GUI (the Claude
desktop app, Codex.app, an IDE) do not read `~/.zshrc`, so their kit MCP
servers start unauthenticated. If that surfaces, the fix is a `launchctl
setenv` shim or the app's own environment — not re-inlining secrets into
`.mcp.json`.

## Known gaps (do not treat as done)

1. **Two servers will not travel.** `mail-index` declares an absolute path
   (`/Users/astemarie/code/mail-index/bin/mcp.mjs`), and `twenty` points at
   `http://100.71.109.66:3020/mcp`, which is reachable only from inside the
   tailnet. Both work on this Mac and will fail on a device that lacks the
   checkout or the tailnet. `mail-index-cloud` covers the first case as a
   hosted alternative; Twenty has no public vhost on the VPS today.

   Removed rather than carried: `netlify` (unused, failed to start) and
   `open-design` (its app is not installed, so the command was a guaranteed
   ENOENT on every device).
2. **Codex is still on local copies.** The migration above was applied to
   Claude Code only. Codex has its own 15 curated skills in `~/.codex/skills`
   and its own `mcp_servers` in `config.toml`; neither has been reconciled
   against the plugin yet. Do that before calling the migration finished.
3. **Onboarding registration is implemented but not released.** CommonKit's
   guided setup now validates the Claude and Codex SessionStart hook manifests,
   registers the marketplace with every installed client, and installs the
   plugin. Ship the CommonKit CLI and kit plugin changes together before
   treating this as available on a fresh device.
4. **USG-139 (Urgent, open):** O(n²) receipts, described above.
5. **USG-137 (Urgent, open):** the daemon reports `state: "healthy"` while
   `lastDriftCheckUnixMs` is null and `verify` fails. `activeTarget` and
   `activeLoadout` are hardcoded `None`. Health is asserted, not measured —
   do not trust a green reading until this is fixed.
6. **Second device (VPS) is not registered.** Only this Mac is a known
   target.

## How to verify on a fresh device

Run these and check the output, do not infer success from a clean install:

```sh
# the plugin is registered
codex plugin list | grep commonkit-kit        # or: claude plugin list

# the payload actually landed
ls ~/.codex/plugins/cache/commonkit/commonkit-kit/0.1.0/skills | wc -l   # expect 58
ls ~/.codex/plugins/cache/commonkit/commonkit-kit/0.1.0/agents | wc -l   # expect 14

# no secret literals were committed
grep -rE 'sk-[a-zA-Z0-9]|Bearer [a-zA-Z0-9]' ~/.commonkit-kit/plugin/.mcp.json   # expect no matches
```

Then start a session and confirm a `commonkit-kit:` skill and a
`commonkit-kit:` agent both appear. A successful install with nothing loaded
in-session means the agent did not restart.

## Updating the kit

The plugin is pinned to a commit. After changing `~/.commonkit-kit`:

```sh
cd ~/.commonkit-kit && git add -A && git commit && git push
codex plugin marketplace add al-unsoldgroup/commonkit   # refresh snapshot
```

Then reinstall the plugin on each device. There is no auto-update.
