# Session Board web

Static, framework-free PWA for the Session Board hub. It is designed for an iPad in landscape and uses SSE for live state plus explicit HTTP posts for human decisions.

```sh
pnpm --filter @commonkit/session-board-web test
pnpm --filter @commonkit/session-board-web build
```

The build writes `dist/`, which the hub serves by default. On the first Approve, Decline, or option tap, the board asks for the hub action token and keeps it in this PWA's local storage. No decision is sent without a tap.
