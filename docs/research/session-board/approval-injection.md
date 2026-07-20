# Programmatic approve/decline of Claude Code & Codex sessions in Orca terminals

Ground-truthed locally on this Mac 2026-07-20. `claude` 2.1.215, `codex-cli` 0.144.6, `orca` CLI.
Confidence: HIGH = doc/source or observed locally; MED = single third-party source; LOW = inferred.

## TL;DR recommendation per agent

- **Claude Code → PreToolUse hook, not keystrokes.** A `PreToolUse` hook resolves the
  decision *before* the TUI ever blocks; shell out to the dashboard's HTTP endpoint and
  return `permissionDecision: allow|deny`. Works in interactive TUI. No scraping, no race.
  (HIGH)
- **Codex CLI (plain TUI) → keystroke injection** via `orca terminal send`. Codex hooks can
  only *deny*, never grant (MED), and `notify` never fires on approval requests. Full
  programmatic approve needs Codex run under `app-server`/`mcp-server` (JSON-RPC elicitation),
  which is not a plain TUI session. For existing TUI sessions, inject number keys. (MED)
- **Detection** for both: `orca terminal wait --for tui-idle` + an output-pattern regex on the
  prompt box. Claude also emits a `Notification`/pre-permission hook event you can push from. (MED)

---

## 1. Claude Code interactive permission prompt

**On screen (HIGH it exists / LOW exact glyphs — not in public docs, redraws are noisy):**
A bordered box, roughly:
```
Do you want to proceed?
❯ 1. Yes
  2. Yes, and don't ask again for <cmd/dir> this session
  3. No, and tell Claude what to do differently (esc)
```
Answered by **number key** (1/2/3, selects immediately) or arrow + Enter; `Esc` = decline.
Left/Right arrows cycle dialog tabs (doc-confirmed). While *working* Claude shows an animated
spinner ("… esc to interrupt"); the spinner stopping + this box is the blocked signal.

**Can `orca terminal send` inject the keystroke?** Yes, mechanically:
`orca terminal send --terminal <h> --text "1" --enter` (approve) / `"3"` (decline).
`send` exposes only `--text`, `--enter`, `--interrupt` — no arbitrary escape/keycodes, so
arrow-navigation is out; rely on the number-key path. Reliable only with a read-verify-send
loop (see §6). **But you should not need this for Claude — use the hook (§2).**

## 2. Claude Code hooks / permission-prompt-tool (the real mechanism)

- **`PreToolUse` hook — auto approve/deny without the TUI. HIGH, works in interactive TUI.**
  Hook returns:
  ```json
  {"hookSpecificOutput":{"hookEventName":"PreToolUse",
    "permissionDecision":"allow","permissionDecisionReason":"..."}}
  ```
  `permissionDecision`: `allow` | `deny` | `ask`. The hook is a command; have it POST the
  tool_name + tool_input to the dashboard and block on the reply, then emit the JSON. This
  turns "TUI blocked on approval" into "dashboard round-trip" — the box never appears.
  Input payload the hook receives: `{session_id, transcript_path, hook_event_name, tool_name,
  tool_input, cwd}`.
- **Detection hook:** a pre-permission event (`Notification`, and per subagent a
  `PermissionRequest` event — MED on exact name) fires when Claude needs approval/attention.
  Register it to mark the session BLOCKED on the dashboard in real time. There is no dedicated
  "idle" event — infer idle from absence of `PostToolUse`/`UserPromptSubmit`.
- **`--permission-prompt-tool <mcpTool>`: print/headless mode ONLY (`-p`). Does NOT apply to
  interactive TUI. HIGH.** Not present in this build's `--help` (SDK/headless feature). MCP tool
  returns `{behavior:"allow"|"deny", updatedInput?, message?}`; cannot allow tools flagged
  `requiresUserInteraction` (coerced to deny, v2.1.199+).
- **Agent SDK `canUseTool(toolName, input, {signal, suggestions})` → `{behavior, updatedInput?,
  message?}`: SDK-driven sessions ONLY, not the `claude` TUI. HIGH.**
- **`--input-format stream-json`: print mode only; no documented channel to inject an approval
  into a live interactive TUI. HIGH.** TUI stdin is the TTY; do not fight the user's keystrokes.

**Net:** for interactive Claude in an Orca terminal, the hook is the clean path; keystroke
injection is the fallback if hooks can't be installed on that session.

## 3. Codex CLI approval

Modes (this build, `-a/--ask-for-approval`): `untrusted` | `on-request` | `never`; sandbox
`-s`: `read-only` | `workspace-write` | `danger-full-access`. `--full-auto` = never+workspace-write.
Legacy names: suggest/auto-edit/full-auto.

**Prompt (MED — number-key box like Claude's):**
```
Allow Codex to run `git push`?
❯ 1. Yes, proceed
  2. Yes, and don't ask again for `git` (writes a prefix_rule to ~/.codex/rules/default.rules)
  3. No, and tell Codex what to do differently
```
Number key or arrow+Enter; Esc declines. Observed locally: Codex TUI also has a `/approve`
command ("approve one retry of a recent auto-review denial") — unrelated to per-command gating.

**Injecting keystrokes:** same as Claude — `orca terminal send --text "1" --enter`. MED reliability.

**Programmatic (non-keystroke) paths:**
- **`notify` config: fires ONLY on `agent-turn-complete`, NOT on approval requests.** Confirmed
  by docs + open issue openai/codex#11808. Payload: `{type, thread-id, turn-id, cwd,
  input-messages, last-assistant-message}`. Useful for "turn done", useless for "blocked on
  approval". (HIGH)
- **`hooks.json` `PreToolUse`: can DENY, cannot GRANT** ("hooks can only deny, never grant") —
  so it can auto-decline but not auto-approve. Events: SessionStart, UserPromptSubmit,
  PreToolUse, PostToolUse, Stop. (MED — third-party doc)
- **`app-server` / `mcp-server` (JSON-RPC): the real programmatic approve path.** Running Codex
  under `codex app-server` or `codex mcp-server` surfaces exec/patch approval requests as
  elicitation the client answers (approved/denied/abort); `thread/start` takes `approvalPolicy`
  + `approvalsReviewer`. Source dirs: `codex-rs/mcp-server/src/exec_approval.rs`,
  `patch_approval.rs`, `codex-rs/protocol/src/approvals.rs`. **Requires launching Codex this way
  — a plain interactive TUI in an Orca terminal is NOT driven by app-server, so this can't
  retrofit an already-running TUI session.** (MED)

## 4. Orca decision gates — separate mechanism

`orca orchestration gate-create --task <id> --question ... [--options json]` /
`gate-resolve --id <gate> --resolution ...` / `gate-list`. These block an **orchestration task**
(agent-to-agent DAG), not an agent's own permission prompt. No integration with Claude/Codex
tool-permission prompts. Use gates to model a human/dashboard decision in an Orca-coordinated
workflow; they do not read or answer the in-TUI approval box. (HIGH, from `--help`)

## 5. Detection — is a session BLOCKED on approval?

Primary signal for both agents: `orca terminal wait --for tui-idle --timeout-ms N` (the working
spinner stops when blocked). tui-idle alone is ambiguous (also true when done), so combine with
an output-pattern check on the last frame via `orca terminal read --json`
(`result.terminal.tail` = array of lines; also `.status`, `nextCursor`).

Regex on the tail (case-insensitive, tolerate box glyphs/spaces):
- Claude Code: `/Do you want to proceed\?/` OR `/❯?\s*1\.\s*Yes\b/` OR `/don't ask again/`
- Codex CLI:   `/Allow Codex to run/` OR `/Yes,\s*proceed/` OR `/don't ask again for/`
- NOT-blocked noise to exclude: spinner frames (`/Working|esc to interrupt/`), and Codex's
  `Goal blocked \(\/goal resume\)` — that's the goal system, **not** an approval prompt (observed
  live). 

Better-than-scraping signals when available:
- Claude Code: the pre-permission hook event (§2) — push-based, exact, no polling. (MED)
- Codex: none for approval (notify excludes it); scraping is the only signal for a plain TUI. (HIGH)

Read shape gotcha: `orca terminal read` returns `{result:{terminal:{tail:[...lines], status,
oldestCursor,nextCursor,latestCursor}}}`; stale handles error `terminal_handle_stale` — re-list
to refresh handles.

## 6. Risks of keystroke injection + mitigations

- **Redraw race:** TUIs repaint continuously (captured frames are full of `Worki•Workin•Working`
  spinner garbage). Sending mid-repaint can land on the wrong widget. → **Read-verify-send loop:**
  (1) `wait --for tui-idle`; (2) `read` and regex-confirm the prompt box + which option is which;
  (3) only then `send --text "<n>" --enter`; (4) `read --cursor <prev>` to confirm the box cleared.
- **Wrong default via Enter:** bare Enter selects the highlighted option (usually Yes/approve) —
  never send `--enter` alone; always send the explicit number so decline is unambiguous.
- **Multi-prompt queue:** a turn can raise several approvals back-to-back. Loop until the prompt
  regex no longer matches after send; don't fire-and-forget one keystroke per detection.
- **Focus / wrong pane:** `send` targets a terminal handle, not screen focus, so foreground focus
  is a non-issue — but a stale handle silently misfires; re-list handles before a send.
- **User contention:** if a human is also typing in that TTY, injected keys interleave. Prefer the
  hook path (Claude) which needs no TTY; gate keystroke injection to sessions marked
  dashboard-controlled.
- **Option-order drift:** don't hardcode "1=yes" blindly across versions — verify the label next
  to the number in the read frame before mapping approve/decline.

### Practical stance
Claude Code: install a `PreToolUse` (+ notification) hook that calls the dashboard → clean,
race-free, bidirectional. Keystroke injection only as fallback.
Codex CLI: for sessions you launch, run under `app-server`/`mcp-server` for true programmatic
approval; for already-running TUI sessions, read-verify-send number keys with the loop above.
