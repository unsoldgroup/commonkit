# Session Board Mac reporter

The reporter runs on a Mac Target. It reads Orca session state, normalizes it to the shared board protocol, and dials outbound to the hub. It never opens an inbound listener.

## Configure and run

Use environment variables:

```sh
export SESSION_BOARD_HUB_URL=https://board.unsold.cloud
export SESSION_BOARD_MACHINE_TOKEN='<machine token>'
export SESSION_BOARD_MACHINE_ID=studio
export SESSION_BOARD_MACHINE_NAME='Studio Mac'
pnpm --filter @commonkit/session-board-reporter start
```

Alternatively, set `SESSION_BOARD_CONFIG` to a JSON file containing `hubUrl`, `machineToken`, `machineId`, and `machineName`. The default path is `~/.config/commonkit/session-board-reporter.json`. This package does not create or modify global agent configuration.

The primary source reads `~/Library/Application Support/Orca/orca-runtime.json`, connects to its WebSocket endpoint with the runtime token, and probes the JSON-RPC command framing. If the runtime rejects that framing, is unreachable, or times out, the reporter automatically falls back to `orca worktree ps --json` every two seconds. Every hub reconnect sends a full snapshot before subsequent deltas.

Open orchestration gates are polled with `orca orchestration gate-list`. A gate is resolved only after the hub sends a decision originating from a human board tap. Codex keystroke execution is an explicit unsupported stub pending UNS-1305. Nothing auto-approves. Claude permission handling belongs to the separate fail-open hook: timeout or hub failure must return control to the normal TUI prompt.

## Refresh the captured fixture

The checked-in fixture matches the verified `worktree ps` envelope recorded in `docs/research/session-board/orca-cli-ground-truth.md`. A fresh live capture was unavailable during UNS-1303 because Orca returned `runtime_unavailable`. With Orca.app running and at least one agent session visible, replace it with:

```sh
orca worktree ps --json > apps/session-board/reporter/fixtures/worktree-ps.json
pnpm --filter @commonkit/session-board-reporter test
```

Review the capture before committing; Orca output can contain local paths, task titles, prompts, and assistant text.

## Manual smoke test

1. Start Orca.app and open Claude or Codex in an Orca worktree.
2. Start a local hub with a reporter token for this machine.
3. Set the variables above to the local hub URL and token, then start the reporter.
4. Confirm the hub `/state` response contains the machine and normalized session.
5. Stop the hub, change agent state, restart the hub, and confirm the reporter reconnects and sends a complete current snapshot.
6. Create an Orca orchestration gate and confirm it appears as an `orca-gate` action. Tap a choice on the board and confirm `orca orchestration gate-list --json` reports it resolved.
7. Stop Orca.app or invalidate the runtime descriptor and confirm the reporter logs fallback selection and continues polling the CLI.

Do not test by approving a live tool automatically. Every approval must remain a human tap.
