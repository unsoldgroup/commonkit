# Pasted-research verification — Orca session dashboard

Adversarial fact-check of the pasted writeup. Verdicts: CONFIRMED / WRONG / UNVERIFIABLE.
Verified 2026-07-20 against public docs + local `orca` CLI (v resolves to `/usr/local/bin/orca`).

## Claim 1 — onorca.dev/docs pages and their content — CONFIRMED

All four cited pages exist (no 404):
- `onorca.dev/docs/model/agents-sessions`
- `onorca.dev/docs/cli/overview`
- `onorca.dev/docs/cli/reference`
- `onorca.dev/docs/model/worktrees`

Specific quotes verified on `model/agents-sessions`:
- Agent session defined verbatim as **"one CLI agent running in one terminal in one worktree."** ✔
- State dots: **green pulsing = actively working, yellow = waiting on input, gray = idle.** ✔
- Restart: exited agent shows a **Restart chip**; one click rehydrates the same agent. ✔ (writeup's "restart indicators")
- State detection: **"State is detected from the terminal's OSC title sequence."** ✔ (writeup's "terminal title updates")
- "Worktree cards surface which agents need attention first": ✔ on agents-sessions ("each worktree card ... inlines its agent sessions with the same colored status dot"; "a sidebar status overlay surfaces the worktrees that need attention first"). Note: the `model/worktrees` page itself does NOT phrase it that way (only "status bar shows agent activity inline; unread worktrees are bolded"). Attribution is slightly loose but substance holds.

## Claim 2 — `orca worktree ps --json` and `orca terminal list --json` — CONFIRMED (with a caveat that hits the architecture)

Both flags exist and emit JSON — confirmed locally and in `docs/cli/reference` / `docs/cli/overview`.

- `orca worktree ps --json` returns per-worktree: `workspaceStatus`, `liveTerminalCount`, `hasAttachedPty`, `unread`, `lastActivityAt`, `lastOutputAt`, `preview`, plus linked issue/PR fields.
- `orca terminal list --json` returns per-terminal: `handle`, `ptyId`, `worktreeId`, `title`, `connected`, `writable`, `lastOutputAt`, `preview`.

**Caveat (load-bearing for the build):** neither JSON payload exposes the semantic agent state (active / waiting-on-input / idle) as a field. The green/yellow/gray dot is derived in the Orca UI from the OSC title sequence; the CLI hands you the raw `title` + `preview` + `lastOutputAt`, not a normalized state enum. A dashboard must re-implement the OSC-title → state mapping itself. The writeup's "output session state suitable for a dashboard" overstates this — it's activity signals, not a ready-made state field.

## Claim 3 — orcastrator.dev vs mcpservers.org/.../orca-cli — WRONG (two different products conflated)

- **orcastrator.dev is a DIFFERENT, unrelated product** that merely shares the name "Orca." It is "an observable agent-lane harness" that dispatches lanes to Codex/Claude/Cursor (GitHub `ratley/orca`). It has NOTHING to do with the onorca.dev coding editor / worktrees / terminals. Citing it as "Orca CLI Reference" for the dashboard is a mis-citation — flag.
- **mcpservers.org/agent-skills/stablyai/orca-cli DOES describe the same onorca.dev product** — worktrees, folder contexts, terminals, automations, the embedded browser, and the `orca` vs `orca-ide` binary rule. This matches the real bundled `orca-cli` skill. It is correct.

Net: the writeup treats two same-named-but-unrelated "Orca" projects as one source of truth. Any command/schema detail sourced from orcastrator.dev is untrustworthy for this build.

## Claim 4 — external iPad/SSH sources — mostly CONFIRMED, one WRONG (non-load-bearing)

- `termai.sh/blog/tailscale-ssh` — exists, titled "Tailscale SSH on mobile: the full setup guide (2026)", dated June 11 2026. ✔
- `huma.id/tailscale-ios` — exists, real blog by Humaid Alqasimi (Tailscale + SSH from iOS, 2022). ✔
- App Store **meshTerm** (id6761196011, "SSH FTP via Tailscale") — real, live on App Store. ✔
- App Store **NovaAccess** (id6749938291, "Tailnet Terminal", GalaxNet Ltd, now NovaScale) — real, live on App Store. ✔
- `emcfarlane.github.io/ipad-dev` — **404 / does not exist.** WRONG, but minor/non-load-bearing.

## Claim 5 — recommended architecture — HOLDS, but misses three things

Architecture (daemon polls CLI JSON → normalized state → web UI tiles → action buttons via `orca terminal send`) is broadly sound. `orca terminal send` and `orca terminal wait` both exist. Gaps the writeup under-weighted:

1. **No semantic state in the JSON (see Claim 2).** The daemon must replicate the OSC-title state machine to produce green/yellow/gray. This is the single most important build detail and the writeup glosses it.
2. **Approval / permission injection is fragile.** "Action buttons via terminal send" for approving a permission prompt means blind keystroke injection into an interactive agent TUI (Claude Code / Codex prompts). Ordering/timing is racy; there is no documented structured "answer the approval" API. Treat as best-effort, not a first-class action.
3. **Poll-only; no event stream / decision gates.** The docs expose polling (`ps`, `list`) and per-terminal `wait` on a condition, but no push/subscribe event stream. Dashboard latency and poll cadence are a real design axis the writeup ignores. Also: per repo convention, writes to *agent* terminals should route through the `orchestration` path, not raw `orca terminal send` — the writeup's action-button design conflicts with that.

## Bottom line

Nothing in the core Orca claims is fabricated — the product, docs, CLI flags, and JSON all check out. The one substantive error is Claim 3 (orcastrator.dev is a different project) and the recurring blind spot that the CLI JSON does not hand you a ready-made agent-state field.
