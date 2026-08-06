# Session Board web

Static, framework-free PWA for the Session Board hub. It is designed for an iPad in landscape and uses SSE for live state plus explicit HTTP posts for human decisions.

```sh
pnpm --filter @commonkit/session-board-web test
pnpm --filter @commonkit/session-board-web build
```

The build writes `dist/`, which the hub serves by default. On the first Allow, Always, Deny, or numbered-option tap, the board asks for the hub action token and keeps it in this PWA's local storage. Always previews its project-local rule before a second confirming tap. No decision is sent without a human tap.
