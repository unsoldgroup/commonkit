# iPad-first realtime dashboard PWA over Tailscale — research

Scope: home-screen standalone PWA on iPadOS, served from a Mac over Tailscale, showing
status cards for AI coding sessions with tap-to-approve and drag-to-regroup, auto-updating
(1-5s freshness). Single user, LAN/tailnet only.

Confidence legend: **High** = corroborated by primary docs / spec / multiple sources;
**Med** = single decent source or reasoned inference; **Low** = weak/uncertain.

Local environment verified at research time: Tailscale CLI `1.98.9` (`/usr/local/bin/tailscale`,
GUI at `/Applications/Tailscale.app`); Node `v25.5.0`, Bun `1.3.11`, pnpm `10.28.2`; repo is a
mixed monorepo — pnpm workspace (`apps/`, `packages/`) + Rust crates (`crates/`, `Cargo.toml`).

---

## 1. iPadOS PWA (home-screen standalone) gotchas

**Standalone install is just a shortcut in a browser-chrome-less window.** It runs WebKit,
same engine limits as Safari. It is fine as an always-on wall dashboard *if you handle
suspension and reconnection explicitly* — the OS will not do it for you. (Med)

**Background suspension / tab freezing.** When the PWA is not foreground, iOS aggressively
suspends JS timers, fetch, and sockets to save battery; background sync and periodic
background updates are unreliable-to-unsupported. Do **not** rely on anything happening while
the app is backgrounded — treat "went background → came back" as "reconnect from scratch".
(High — [magicbell](https://www.magicbell.com/blog/pwa-ios-limitations-safari-support-complete-guide),
[firt.dev pwa-ios](https://firt.dev/notes/pwa-ios/))

**WebSocket/SSE dies on background.** Documented failure mode: switch away briefly, the socket
closes — commonly WS close code **1005 (No Status Received)** — and you miss the event sequence
in between. This is a real, reproduced issue (incl. an openclaw report of exactly this in a
PWA). Plan for silent disconnects, not graceful ones. (High —
[openclaw#2672](https://github.com/openclaw/openclaw/issues/2672),
[websocket.org reconnection](https://websocket.org/guides/reconnection/))

**Service worker limits.** SW runs but no reliable background execution; script-writable storage
capped and evicted after ~7 days unused; total quota small (~50MB) and can be cleared after
weeks of non-use. For an always-on dashboard the SW is worth having only for the installable
manifest + offline shell, **not** as a data pipeline. Don't architect realtime through the SW.
(High — magicbell, firt.dev)

**Viewport / safe-area.** Must set `<meta name="viewport" content="width=device-width,
initial-scale=1, viewport-fit=cover">`. Without `viewport-fit=cover`, all `env(safe-area-inset-*)`
resolve to 0 and Safari letterboxes in landscape. Pad the fixed frame with
`env(safe-area-inset-*)` on all four sides (landscape iPad + Dynamic Island/notch report
different insets per orientation). (High —
[WebKit iPhone X](https://webkit.org/blog/7929/designing-websites-for-iphone-x/),
[dev.to handsome PWAs](https://dev.to/karmasakshi/make-your-pwas-look-handsome-on-ios-1o08))

**Keeping the screen on.** Screen Wake Lock API works in installed PWAs only on **iOS/iPadOS
18.4+** (a long-standing installed-PWA bug was fixed there); pre-18.4 it fails in standalone.
The lock auto-releases when the app is backgrounded, so re-request it on `visibilitychange →
visible`. For a wall display, also set the iPad's Auto-Lock to Never as the real backstop —
don't depend on Wake Lock alone. (High —
[whatpwacando wake-lock](https://whatpwacando.today/wake-lock/), magicbell)

**Recovery pattern production dashboards use (implement this):**
- Listen to `visibilitychange`; when `document.visibilityState === 'visible'`, if the transport
  is not OPEN, reconnect immediately (don't wait for backoff). Also listen to `online`.
- On (re)connect, **refetch full state** (a `GET /state` snapshot) rather than trusting you
  can resume a stream — you cannot assume no events were missed while suspended.
- If using a resumable stream, send `Last-Event-ID` (SSE does this automatically) so the server
  can replay; but still reconcile against a full snapshot on wake.
- Exponential backoff (with cap + jitter) only for the *repeated-failure* case, not the first
  wake reconnect. (High — [oneuptime](https://oneuptime.com/blog/post/2026-01-24-websocket-reconnection-logic/view),
  websocket.org)

---

## 2. Realtime transport: SSE vs WebSocket vs polling

Need is server→client push, 1-5s freshness, one user, over Tailscale HTTPS. The approve action
is a low-frequency client→server event — a plain `POST` handles it; it does **not** require a
bidirectional socket.

**Recommendation: SSE (EventSource).** (High-confidence recommendation)
- Server→client only is exactly SSE's shape; approvals go over ordinary `POST` requests.
- `EventSource` has **built-in auto-reconnect** and `Last-Event-ID` replay — half the
  reconnection code you'd otherwise write for WS is free. iOS Safari has supported it for years.
  ([caniuse eventsource](https://caniuse.com/eventsource), [MDN/spec](https://html.spec.whatwg.org/multipage/server-sent-events.html))
- Plain HTTP → passes through `tailscale serve`'s HTTPS reverse proxy with zero special config.
- HTTP/2 removes the old 6-connections-per-origin SSE cap via multiplexing; you have one stream
  anyway, so this is a non-issue here, but `tailscale serve` terminates TLS and speaks HTTP/2 so
  you get it for free. ([dev.to SSE/HTTP2](https://dev.to/abhivyaktii/understanding-server-sent-events-sse-and-why-http2-matters-1cj7))

Caveats that **don't** bite this setup: SSE's classic weakness is corporate proxies/CDNs
silently buffering the stream — irrelevant on a direct LAN/tailnet path with no CDN.
([softwaremill](https://softwaremill.com/sse-vs-websockets-comparing-real-time-communication-protocols/))

**Why not WebSocket:** bidirectional capability you don't need, and you'd hand-roll the
reconnect/backoff/replay that SSE gives you. Pick it only if you later add high-rate
client→server streaming (e.g. live terminal input). (High)

**Why not short polling:** simplest of all and genuinely fine as a fallback (a 2-3s `GET /state`
poll is robust and survives every suspension cleanly). It's a defensible lazy default; SSE wins
because it's lower-latency and the client already needs a full-state refetch on wake, which
doubles as the poll. Keep polling in your pocket as the fallback transport. (Med)

---

## 3. Tailscale serve specifics (macOS, 2025/2026)

**Serving a port works on every macOS variant** — App Store, Standalone, and open-source.
The documented limitation is only that App Store / Standalone variants can serve **ports but not
files/directories**, and can have multi-interface routing quirks. Since the plan is to serve an
HTTP port from the local Node/Bun server, this is fully supported as-is. (High —
[Tailscale Serve docs](https://tailscale.com/docs/features/tailscale-serve))

**Current syntax** (proxy a local HTTP port to tailnet HTTPS, run in background):
```sh
# One-shot foreground:
tailscale serve <localPort>            # e.g. tailscale serve 8787
# Background (persistent) form:
tailscale serve --bg <localPort>
# Explicit HTTPS listen port + target:
tailscale serve --bg --https=443 http://127.0.0.1:8787
tailscale serve status                 # inspect
tailscale serve --https=443 off        # tear down
```
Access becomes `https://<machine-name>.<tailnet>.ts.net/`. (Med — CLI shape per
[serve reference](https://tailscale.com/docs/reference/tailscale-cli/serve); verify exact flags
with `tailscale serve --help` on 1.98.9, which is installed.)

**TLS cert provisioning.** Enable **HTTPS** for the tailnet in the admin console (MagicDNS +
HTTPS toggle). Tailscale then auto-provisions a Let's Encrypt cert for the machine's
`*.ts.net` name; `tailscale serve` terminates TLS using it automatically. You can also run
`tailscale cert <name>.ts.net` to materialize the cert, but `serve` handles it transparently.
(High — [Enabling HTTPS](https://tailscale.com/docs/how-to/set-up-https-certificates))

**iPad + Safari trust of `ts.net` certs.** Because the cert is a real Let's Encrypt cert for a
public `*.ts.net` hostname, iPad Safari trusts it with **no profile install and no cert
warning** — provided the iPad is on the same tailnet (Tailscale app installed + logged in) so
MagicDNS resolves the name. This is the key reason to use `serve` HTTPS rather than a raw
`http://100.x.y.z:port` (which works but is non-TLS and can't be a home-screen PWA origin
cleanly). (High — inference from Let's Encrypt public cert + MagicDNS; standard Tailscale HTTPS
behavior)

**Local variant note:** the installed CLI is `/usr/local/bin/tailscale` with GUI
`/Applications/Tailscale.app`. Serving a port needs no variant change. If you ever hit the
multi-interface routing quirk (endpoint not in default route table), the open-source
`tailscaled` userspace-networking variant is the escape hatch — unlikely to be needed for a
single Mac serving localhost.

---

## 4. Touch drag-and-drop for card regrouping

**Native HTML5 drag-and-drop on touch: still bad, don't use it directly.** It has no reliable
touch support, no axis lock, no container bounds, no custom drag preview. Confirmed current
state. (High — [dnd-kit discussion](https://github.com/clauderic/dnd-kit/discussions/306),
[studyraid](https://app.studyraid.com/en/read/12149/389953/advantages-of-using-dnd-kit-over-native-html5-drag-and-drop))

Two credible libraries in 2026:

- **@atlaskit/pragmatic-drag-and-drop** — framework-agnostic core (~4.7kB), headless, works with
  vanilla JS or any view layer. It *is* built on the native HTML5 DnD API and "detects touch
  events out of the box." Caveat: because it wraps native DnD, historical touch weakness on
  mobile is a real risk to test on the actual iPad; Atlassian ships an optional touch/pointer
  adapter for exactly this. Great fit for a vanilla/lightweight app. (Med-High —
  [pragmatic-drag-and-drop repo](https://github.com/atlassian/pragmatic-drag-and-drop),
  [Atlassian design](https://atlassian.design/components/pragmatic-drag-and-drop))
- **@dnd-kit** — deliberately **not** built on HTML5 DnD; first-class mouse/touch/pointer/keyboard
  support, axis lock, container bounds, custom collision + preview. Best-in-class touch. But it's
  React-oriented. (High — [dndkit.com](https://dndkit.com/))

**Recommendation:**
- If the app stays vanilla / non-React → **pragmatic-drag-and-drop core** (framework-agnostic,
  tiny), and verify touch drag on the physical iPad early; add its pointer adapter if native
  touch is flaky.
- If you'd reach for React anyway → **@dnd-kit** for the strongest touch behavior with the least
  fighting.
- Laziest sufficient option for simple "drag card between N columns": a small custom
  **Pointer Events** handler with `setPointerCapture` (~30-50 lines) beats pulling a lib for one
  interaction. Pointer capture keeps events routed to the dragged card regardless of finger
  position — the one hard part of touch DnD. Escalate to a library only when you need
  sortable-within-group, animations, or accessibility niceties. (Med —
  [javascript.info pointer-events](https://javascript.info/pointer-events))

---

## 5. Server stack for the Mac

Requirements: poll a CLI, serve API + static assets, push SSE updates. One process, no build
step, lives in a pnpm/TS monorepo that also has Rust crates.

**Recommendation: Bun, single process, Hono optional.** (Med-High)
- Bun runs TS **directly** (no build/transpile step) — satisfies "no build complexity". Node v25
  can run TS too now, but Bun's zero-config TS + built-in bundler/test/pm is the smoother
  single-binary story for a small always-on local server. (High —
  [strapi bun-vs-node](https://strapi.io/blog/bun-vs-nodejs-performance-comparison-guide))
- **Hono** is the pragmatic router: tiny, standards-based (`Request`/`Response`), first-class on
  Bun, `serveStatic` middleware for the PWA assets, and SSE helper included. Perf is irrelevant
  at one user, but Hono keeps routing + static + SSE in ~one small file. (High —
  [Hono Bun docs](https://hono.dev/docs/getting-started/bun),
  [oler.pages.dev](https://oler.pages.dev/blog/hono-with-bun-for-new-projects/))
- Polling the CLI: `Bun.spawn` / `child_process` on an interval, hold latest state in memory,
  fan out over the SSE endpoint. No DB, no queue — a module-level object is the store.
- **Ladder check:** could be `Bun.serve` with a hand-written router and no Hono at all if routes
  stay trivial (`/`, `/state`, `/events`, `/approve`). Take Hono only once you have >~4 routes or
  want its SSE/static helpers. `plain node:http` is the fallback if you specifically want to avoid
  even a Bun dependency, but you already have Bun installed and the monorepo is pnpm/TS — Bun is
  the lower-friction choice.

**Rust axum — overkill here.** (High) The repo having Rust crates is not a reason to write the
dashboard server in Rust. axum buys nothing at one-user/1-5s freshness and costs you a compile
step, a second toolchain in the serving path, and slower iteration on a glorified CLI-poller +
SSE fan-out. Justified only if: (a) the "poll the CLI" part is actually deep integration with an
existing Rust crate's in-process API (then a thin axum shim avoids shelling out), or (b) you want
a single static binary to deploy elsewhere. Neither is stated. Keep the server in Bun/TS next to
the frontend; call the Rust crates as CLIs if needed.

---

## TL;DR recommendations

1. **Transport:** SSE (`EventSource`) for push + `POST` for approvals. Auto-reconnect + replay for free.
2. **iPad resilience:** on `visibilitychange→visible`, reconnect immediately and **refetch full
   `/state`** (assume events were missed while suspended). Re-request Wake Lock (18.4+) and set
   Auto-Lock=Never on the iPad. `viewport-fit=cover` + `env(safe-area-inset-*)`.
3. **Tailscale:** `tailscale serve --bg <port>`; enable tailnet HTTPS for auto Let's Encrypt cert;
   iPad Safari trusts `*.ts.net` with no profile. Any macOS variant serves ports fine.
4. **Drag:** pragmatic-drag-and-drop (vanilla) or @dnd-kit (React); or ~40 lines of Pointer Events
   with `setPointerCapture` for a simple N-column regroup. Never raw HTML5 DnD on touch.
5. **Server:** Bun + optional Hono, one process, TS run directly, in-memory state, `Bun.spawn` to
   poll the CLI. Rust/axum is overkill unless integrating an in-process Rust crate.
</content>
</invoke>
