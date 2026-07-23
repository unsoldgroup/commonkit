# Session Board Claude hook

`session-board-hook.ts` is a single-file Bun `PreToolUse` hook. It sends permission-relevant tool requests to the reporter on localhost and waits up to 55 seconds for a human board tap. An allow or deny tap returns Claude's structured `permissionDecision` response. A timeout, invalid response, or unreachable reporter prints nothing and exits zero, leaving Claude's normal TUI prompt in control.

Install it with:

```sh
apps/session-board/hook/install-hook.sh
```

The installer merges one hook entry into `~/.claude/settings.json` without replacing existing hooks. Set `CLAUDE_SETTINGS_PATH` to test against another file. The repository never runs this installer automatically.
