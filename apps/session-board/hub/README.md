# Session Board hub

The hub is one Bun process. Reporters dial its `/reporter` WebSocket; the board reads `/state`
and `/events`, and posts human decisions to `/actions/:id/decision`.

## Run

```sh
SESSION_BOARD_REPORTER_TOKENS='{"studio":"replace-me"}' \
SESSION_BOARD_ACTION_TOKEN='replace-me-too' \
bun run apps/session-board/hub/src/main.ts
```

`SESSION_BOARD_REPORTER_TOKENS` maps Target ids to unique bearer tokens. The web action client
uses `SESSION_BOARD_ACTION_TOKEN`. Optional `SESSION_BOARD_HOST` and `SESSION_BOARD_PORT`
default to `127.0.0.1:8787`.

The hub keeps live board state in memory. It stores group layout, the seven-day decision log,
and push subscriptions under `data/`, and serves the PWA from `../web/dist` when that directory
exists.

Web push is enabled only when `SESSION_BOARD_VAPID_PUBLIC`, `SESSION_BOARD_VAPID_PRIVATE`, and
`SESSION_BOARD_VAPID_SUBJECT` are all set. Without them, the rest of the hub continues normally.
