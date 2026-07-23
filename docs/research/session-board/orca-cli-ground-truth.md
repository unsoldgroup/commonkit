# Orca CLI — Session Board Ground Truth

Verified by running `orca` against a live local runtime on 2026-07-20 (macOS, Orca.app pid 14569, schema v1, 202 commands). Confidence tags: **[V]** verified-by-running, **[B]** verified from app bundle (`Orca.app/Contents/Resources/app.asar`), **[I]** inferred.

## 0. TL;DR for the dashboard
- Poll `orca worktree ps --json` — one row per worktree with a rolled-up `status` + per-agent `agents[]`. This is the single richest endpoint for a session board. **[V]**
- Session "needs input" = `agents[].state === "waiting"` (permission/prompt) or worktree `status === "permission"`. **[V/B]**
- No event stream in the CLI; **polling only** (plus blocking `terminal wait`/`orchestration ask`). **[V/I]**
- A dashboard can bypass the CLI: read `~/Library/Application Support/Orca/orca-runtime.json` for the WebSocket endpoint + `authToken` and speak the same JSON-RPC. **[V]**

## 1. Invocation & envelope
- Binary: `/usr/local/bin/orca`. `--json` is a **global** flag; nearly every read command supports it (`status`, `worktree list/ps/show`, `terminal list/show`, `orchestration *`, `project list`, `repo list`, `environment list`). **[V]**
- zsh gotcha: unquoted `$cmd` containing a space is NOT word-split by zsh; the runtime then errors `Unknown command: "worktree list"`. Use `orca worktree list --json` literally, or `${=cmd}`. **[V]**
- JSON envelope (all commands):
  ```json
  { "id": "<uuid|local>", "ok": true, "result": { ... },
    "_meta": { "runtimeId": "<uuid|null>" } }
  ```
  Errors: `"ok": false, "error": {"code","message","data":{"suggestions","nextSteps"}}`. **[V]**

## 2. Command output shapes (real fields)

### `orca status --json` **[V]**
```
result.app     = { running:bool, pid:int, desktopWindowStatus:"available" }
result.runtime = { state:"ready", reachable:bool, runtimeId:uuid }
result.graph   = { state:"ready" }
```

### `orca worktree ps --json` — PRIMARY session-board feed **[V]**
`result = { worktrees:[…], totalCount:int, truncated:bool }`. Each worktree:
```
workspaceKind, worktreeId, worktreeInstanceId, repoId, hostId, repo (name),
path, branch, terminalPlatform,
displayName, comment,
isArchived, isMainWorktree, isPinned, isActive, unread,
parentWorktreeId, childWorktreeIds[],           // hierarchy
sortOrder, manualOrder, createdAt, lastActivityAt, lastOutputAt,
workspaceStatus,      // e.g. "in-progress"  (lifecycle, coarse)
status,               // ROLLUP card status — see §3
liveTerminalCount, hasAttachedPty, hasHostSidebarActivity,
preview (str),        // last terminal output snippet
linkedIssue, linkedPR, linkedLinearIssue, linkedGitLabMR, linkedGitLabIssue,
agents:[ … ]          // per-agent — see §3
```
`agents[]` element:
```
paneKey, parentPaneKey, agentType ("claude"|"codex"),
state,                // "working"|"waiting"|"blocked"|"done"|"idle"|"interrupted"
stateStartedAt, updatedAt, interrupted (bool),
prompt, taskTitle, displayName,
toolName, toolInput,  // current tool call (set while working/waiting)
lastAssistantMessage
```

### `orca worktree list --json` **[V]**
Static metadata (no agent/live state). Fields: `id` (`<repoId>::<path>`), `instanceId`, `repoId`, `projectId`, `hostId`, `projectHostSetupId`, `path`, `head`, `branch`, `isBare`, `isMainWorktree`, `displayName`, `comment`, `isArchived`, `isUnread`, `isPinned`, `sortOrder`, `lastActivityAt`, `workspaceStatus`, `parentWorktreeId`, `childWorktreeIds[]`, `lineage`, many `linked*` (Linear/PR/GitLab/Bitbucket/AzureDevOps/Gitea), nested `git{}`. Use `worktree ps` instead for live state.

### `orca terminal list --json` **[V]**
`result = { terminals:[…], visualLayouts:[…], totalCount, truncated }`. Terminal:
```
handle ("term_<uuid>"),  ptyId,  worktreeId,  worktreePath,  branch,
tabId, leafId,  title (str),  connected (bool), writable (bool),
lastOutputAt (int|null),  preview (str)
```
`title` carries the agent task + a spinner glyph while active (e.g. `"✳ Create lavish plan…"`, `"⠙ insurance-corpus"`) — cosmetic, not a reliable state source. `visualLayouts[]` = tab/split tree per worktree.

### `orca terminal show --terminal <handle> --json` **[V]**
`result.terminal` = terminal-list fields + `paneRuntimeId`, `rendererGraphEpoch`. No richer agent state than `worktree ps`.

### `orca orchestration task-list --json` **[V]**
`result = { tasks:[…], count }`. Task:
```
id ("task_<hex>"), parent_id, deps[] (DAG edges),
created_by_terminal_handle, spec, task_title, display_name,
status ("ready"|"completed"|…), result, created_at, completed_at
```

### `orca orchestration inbox --json` **[V]**
`result = { messages:[…], count }`. Message:
```
id ("msg_<hex>"), from_handle, to_handle, sender_pane_key,
type ("dispatch"|"status"|"heartbeat"|"worker_done"|…),
subject, body, payload, priority, thread_id, sequence,
read (bool), created_at, delivered_at
```

### `orca orchestration gate-list --json` **[V]**
`result = { gates:[…], count }`. Empty in sample (`count:0`). See §4.

### `orca project list --json` / `orca repo list --json` **[V]**
- repo: `id (uuid)`, `name`, filesystem `path`.
- project: `id` (`github:<owner>/<repo>`), `name`, provider ref. Projects are the remote/logical grouping; repos are local checkouts; worktrees carry both `repoId` and `projectId`.

### `orca environment list --json` **[V]**
Remote runtimes: `{ id, name, createdAt, updatedAt, lastUsedAt, runtimeId, preferredEndpointId, endpoints:[{ id, kind:"websocket", label, endpoint:"ws://host:port" }] }`.

## 3. Question (a) & (b): session states & "waiting for input"

**Per-agent state** — `worktree ps → agents[].state` **[V, enum confirmed B]**
Full union: `working`, `waiting`, `blocked`, `done`, `idle`, `interrupted`.
Derivation (Claude Code / Codex hook events, from bundle): **[B]**
- `PreToolUse` (tool needing approval) / `PermissionRequest` / permission `Notification` → **`waiting`**
- active tool use / `UserPromptSubmit` → **`working`**
- `Stop` / `SubagentStop` → **`done`** (`done` + `interrupted` renders "stopped")
- orchestration gate blocking the pane → **`blocked`**
- no activity → **`idle`**

**Rolled-up worktree card** — `worktree ps → status` **[B enum, V partial]**
Union: `active`, `working`, `permission`, `done`, `inactive`. UI label map (bundle):
`{active:"Active", working:"Working", permission:"Needs permission", done:"Done", inactive:"Inactive"}`. Sampled live values: `active`, `working`, `inactive` (no agent was mid-permission at capture).

**Detecting "agent is waiting for user input / permission approval":** **[V/B]**
1. Best signal: any `agents[].state === "waiting"` (permission prompt / needs input). `blocked` = waiting on an orchestration gate.
2. Worktree-level: `status === "permission"`.
3. `tui-idle`: `orca terminal wait --terminal <h> --for tui-idle` blocks until the agent CLI goes idle (used for Claude/Codex/Gemini/Grok). It is a **blocking wait**, not a queryable field — good for "is this agent done churning", not for a poll snapshot.
4. Terminal `title` spinner glyphs indicate active work but are cosmetic — do not parse for state.

`workspaceStatus` (e.g. `in-progress`) is a coarse lifecycle marker, NOT live agent state — don't use it for the board.

## 4. Question (c): decision gates
- Created **only via orchestration**: `orca orchestration gate-create --task <id> --question <q> --options <…>`; resolved with `gate-resolve --id <id> --resolution <…>`; listed with `gate-list [--task] [--status]`. **[V]**
- A gate blocks an orchestration **task** and surfaces to a coordinator/human as a question+options. It is a multi-agent-coordination construct. **[V]**
- **Interactive CLI permission prompts are NOT gates.** A Claude/Codex "allow this tool?" prompt shows up as `agents[].state === "waiting"` / worktree `status === "permission"`, driven by hook events — never as an orchestration gate. Treat the two as distinct signals on the board. **[B]**

## 5. Question (d): events / streaming
- No pub/sub, `watch`, or `--follow` command in the CLI. **[V]**
- Blocking primitives only: `terminal wait --for exit|tui-idle`, `orchestration ask` (blocks until answered), `orchestration run` (coordinator loop), `orchestration check --wait`. These block one call server-side; they are not a general event feed.
- **Dashboard model = poll** `worktree ps --json` (+ `orchestration inbox`/`task-list`/`gate-list`) on an interval. **[V/I]**
- The underlying WebSocket runtime *may* support push/subscriptions, but the CLI does not expose it. **[I]**

## 6. Question (e): identity & grouping
- **Session identity:** a worktree is the unit (`worktreeId` = `<repoId>::<path>`; also `worktreeInstanceId`). An agent within it = `paneKey` (+ `agentType`). A terminal = `handle` (`term_<uuid>`) / `ptyId`. Orchestration actors are addressed by terminal `handle`. **[V]**
- **Grouping:** worktree → `repoId` (local checkout) and `projectId` (`github:<owner>/<repo>`, logical/remote) and `hostId`. Worktrees self-nest via `parentWorktreeId` / `childWorktreeIds[]`. So board hierarchy = project ▸ repo ▸ worktree(s) ▸ agents[] / terminals. **[V]**

## 7. Question 4: HTTP/RPC API a dashboard could hit directly
**Yes — the CLI is a thin client over a local JSON-RPC runtime.** **[V]**

Runtime descriptor: `~/Library/Application Support/Orca/orca-runtime.json`
```json
{ "runtimeId":"…", "pid":14569,
  "transports":[
    { "kind":"unix", "endpoint":"…/orca/o-14569-ef82.sock" },
    { "kind":"websocket", "endpoint":"ws://0.0.0.0:62739" } ],
  "authToken":"<hex>", "startedAt":<ms> }
```
- Live desktop runtime listens on **127.0.0.1:62739** (WebSocket) + a unix socket `o-<pid>-<short>.sock` (both seen via `lsof`). **[V]**
- A dashboard can read this file, connect to the WS transport with `authToken`, and send the same commands the CLI sends, getting the identical `{id,ok,result,_meta}` envelope — no shelling out. **[V/I]** (Exact WS framing/method names not reverse-engineered here; shelling `orca … --json` is the safe, supported path. **[I]**)
- Headless alternative: `orca serve [--port 6768] [--pairing-address …]` prints an endpoint + pairing data; remote runtimes are saved via `orca environment add` and listed with `environment list` (their `endpoints[].endpoint` = `ws://host:port`). **[V]**
- Separate concern: `agent-hooks/endpoint.env` exposes `ORCA_AGENT_HOOK_PORT=49895` + token — that's the inbound hook receiver for agent CLIs (feeds the state machine in §3), **not** the query API. **[V]**

## 8. Config / state locations **[V]**
- `~/.orca/` — `keybindings.json`, `linear-workspaces.json`, `linear-tokens/`, `agent-hooks/`.
- `~/Library/Application Support/Orca/` — `orca-runtime.json`, `agent-hooks/endpoint.env`, `daemon/daemon-v22.sock`, per-runtime `o-<pid>-<hex>.sock`, `orca-{claude,codex}-usage.json`, `orca-stats.json`, `logs/main.trace.ndjson*`.
- Machine-readable command schema: `orca agent-context --json` (202 commands; each: `command, path, aliases, argumentMode, summary, usage, flags[], positionalArgs[], examples[], notes`). Version-matched skill guides: `orca skills list` / `orca skills get orca-cli`.
