# Session Board deployment

Session Board is a tailnet-only hub-and-spoke service. The Bun hub runs on the VPS Target and serves the board through Caddy at `https://board.unsold.cloud`. Each Mac Target runs one outbound reporter connection to the hub. The reporter reads Orca's local JSON-RPC WebSocket first and falls back to `orca worktree ps --json` every two seconds through the same state-source interface.

State travels up to the hub. Decisions travel down to the reporter. An approval is acted on only after a human taps the board. Nothing auto-approves. If the Claude hook cannot reach the reporter or times out, it fails open to Claude's normal TUI prompt.

The install scripts operate only on the Target where they are run. They do not SSH to another Target, create DNS records, or edit Claude or Codex global configuration.

## Prerequisites

Clone this repository on both Targets. Use Bun and pnpm from the same non-root account that will own each service.

On the VPS Target, install:

- Bun, pnpm, Caddy, and a running system Caddy service.
- acme.sh at `~/.acme.sh/acme.sh` for the service user.
- A Cloudflare API token with DNS edit access for the `unsold.cloud` zone (Cloudflare is the authoritative DNS; the Hostinger zone copy is not served).
- `sudo` access for installing the Caddy certificate and site snippet and reloading Caddy.

The main `/etc/caddy/Caddyfile` must import the snippets directory once:

```caddyfile
import /etc/caddy/Caddyfile.d/*.caddy
```

The VPS service is a systemd user service. Enable lingering so it starts at boot without an interactive login:

```sh
sudo loginctl enable-linger "$USER"
```

On each Mac Target, install Bun, pnpm, Orca, and Tailscale. Confirm the Mac can resolve and reach `board.unsold.cloud` over the tailnet before installing the reporter.

## DNS

Create this record in the Cloudflare `unsold.cloud` zone:

| Type | Name | Value | Proxy |
| --- | --- | --- | --- |
| A | `board` | VPS tailnet IPv4 address | DNS only |

The address must be the VPS Tailscale address, not its public address. Access therefore requires the client to be on the tailnet. Do not expose the hub port or this hostname through a public reverse proxy. The installer does not create or change this record. The Cloudflare token passed to acme.sh is used only for the temporary DNS-01 validation record.

## Install the VPS hub

Choose one random action token for the board and a different random reporter token for every Mac Target. Target ids in `SESSION_BOARD_REPORTER_TOKENS` must exactly match each reporter's `SESSION_BOARD_MACHINE_ID`.

From the repository root on the VPS:

```sh
export SESSION_BOARD_REPORTER_TOKENS='{"studio":"replace-with-random-token"}'
export SESSION_BOARD_ACTION_TOKEN='replace-with-a-different-random-token'
export SESSION_BOARD_VAPID_PUBLIC='replace-with-vapid-public-key'
export SESSION_BOARD_VAPID_PRIVATE='replace-with-vapid-private-key'
export SESSION_BOARD_VAPID_SUBJECT='mailto:admin@example.com'
export CF_Token='cloudflare-dns-edit-token'
apps/session-board/deploy/install-vps.sh
unset SESSION_BOARD_REPORTER_TOKENS SESSION_BOARD_ACTION_TOKEN SESSION_BOARD_VAPID_PUBLIC SESSION_BOARD_VAPID_PRIVATE SESSION_BOARD_VAPID_SUBJECT CF_Token
```

The script installs dependencies, builds the web app, writes the hub environment file with mode `0600`, installs and starts `board-hub.service`, explicitly runs the acme.sh DNS-01 challenge, installs the certificate for Caddy, validates the Caddy configuration, and reloads Caddy. It does not create the permanent DNS record.

Generate a VAPID key pair once from the repository root:

```sh
pnpm dlx web-push generate-vapid-keys
```

Keep the private key in the hub environment only. `SESSION_BOARD_VAPID_SUBJECT` is a `mailto:` or `https:` contact URI. Push is optional: if any VAPID variable is absent, the hub returns a null public key and the board continues without push.

Verify:

```sh
systemctl --user status board-hub.service
journalctl --user -u board-hub.service -n 100 --no-pager
curl --fail --show-error https://board.unsold.cloud/state
```

Hub controls:

```sh
systemctl --user stop board-hub.service
systemctl --user start board-hub.service
systemctl --user restart board-hub.service
systemctl --user disable --now board-hub.service
```

The hub token file is `~/.config/commonkit/session-board/hub.env`. Persistent board layout is stored under `apps/session-board/hub/data/` in the repository checkout.

## Board actions, history, and authentication

The approval-card verbs are:

- **Allow**: allow this request once.
- **Always**: allow this request and ask Claude Code to persist the exact rule previewed on the card. The hook returns `updatedPermissions` with destination `localSettings`, so Claude Code writes `<cwd>/.claude/settings.local.json`; the board never edits settings and never writes global configuration (ADR 0009).
- **Deny**: deny the request, optionally with a steer message.
- **Numbered option**: select the displayed Codex option.

Every verdict comes from a human tap. Failures and timeouts return no decision and fall back to the terminal prompt. Deny steer delivery is best-effort and only reaches Claude sessions whose `cwd` matches an Orca-managed worktree; unmatched sessions are skipped silently.

The hub records human decisions for seven days in `data/decisions.json`. The board's Decisions tab reads them from `GET /decisions`; older records are removed as new decisions are appended.

The tailnet is the read boundary. `GET /state`, `GET /events`, `GET /sessions/:id/tail`, `GET /decisions`, and the VAPID public-key read are tokenless. Mutating endpoints are gated: decisions and push subscription registration require either a verified Cloudflare Access identity or the board action bearer token, while reporter writes use that Target's reporter token.

### Cloudflare Access sign-in

A browser session authenticated by Cloudflare Access carries a signed
`Cf-Access-Jwt-Assertion` header and is never prompted for a token. Machine
callers — the reporter and the permission hook — keep their bearer tokens, so
neither needs an Access service token.

The hub verifies the assertion cryptographically against the team JWKS rather
than trusting the header's presence. This matters because Caddy still serves
the hub on the tailnet IP, so a tailnet peer could otherwise forge the header.

Set both variables together, or neither. The hub refuses to start with only one,
because a half-configured Access setup silently leaves browser sessions on the
token prompt:

- `SESSION_BOARD_ACCESS_TEAM_DOMAIN` — for example `unsold.cloudflareaccess.com`
- `SESSION_BOARD_ACCESS_AUD` — the Access application's audience tag

Operator steps, all outside this repository:

1. Add a `cloudflared` ingress rule for the board hostname pointing at
   `http://127.0.0.1:8787`, alongside the existing rules in
   `/etc/cloudflared/config.yml`.
2. Change the board DNS record from a dns-only A record pointing at the tailnet
   IP to a **proxied** CNAME for the tunnel. Access cannot see traffic that never
   reaches the Cloudflare edge, which is why the dns-only record must go.
3. Create the Access application for the board hostname with a **One-time PIN by
   email** policy, and copy its audience tag into `SESSION_BOARD_ACCESS_AUD`.
4. Add an Access **Bypass** rule for `/reporter` so reporter WebSocket traffic is
   unaffected.

Until those steps are done the variables should stay unset, and the board keeps
its existing bearer-token behaviour.

## Push subscriptions

On the first board open per device, the PWA reads `GET /push/vapid-public-key` and offers notification permission when push is configured. It records that the offer was made so a denial or dismissal is not repeatedly prompted. The small bell control in the header shows the device state and can retry opt-in later.

After permission is granted, the service worker creates a browser push subscription with the VAPID public key. The PWA posts that subscription to `POST /push/subscriptions` using the board action token already stored on the device. Subscriptions are stored by endpoint in `data/push-subscriptions.json`. Turning the bell off unsubscribes that browser endpoint; the hub also prunes endpoints when their push service reports them expired.

Each newly seen pending action produces at most one push containing its summary. Opening the notification focuses an existing board window or opens the board. Push delivery is informational only and never approves, denies, delays, or otherwise affects the decision path. Verify delivery manually on each deployed device because browser push services are not exercised in local tests.

## Install a Mac reporter

From the same repository revision on the Mac, set the Target identity and its matching token:

```sh
export SESSION_BOARD_MACHINE_ID='studio'
export SESSION_BOARD_MACHINE_NAME='Studio Mac'
export SESSION_BOARD_MACHINE_TOKEN='replace-with-studio-token'
export SESSION_BOARD_HUB_URL='https://board.unsold.cloud'
apps/session-board/deploy/install-mac.sh
unset SESSION_BOARD_MACHINE_ID SESSION_BOARD_MACHINE_NAME SESSION_BOARD_MACHINE_TOKEN SESSION_BOARD_HUB_URL
```

The script writes `~/.config/commonkit/session-board-reporter.json` with mode `0600`, renders `~/Library/LaunchAgents/cloud.unsold.session-board-reporter.plist`, and bootstraps and enables it with launchd. It does not install the Claude hook or modify `~/.claude/settings.json`; use the separately reviewed hook installer if that Adapter is desired.

Reporter controls:

```sh
launchctl kickstart -k "gui/$(id -u)/cloud.unsold.session-board-reporter"
launchctl bootout "gui/$(id -u)/cloud.unsold.session-board-reporter"
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/cloud.unsold.session-board-reporter.plist"
launchctl enable "gui/$(id -u)/cloud.unsold.session-board-reporter"
```

Inspect status and logs:

```sh
launchctl print "gui/$(id -u)/cloud.unsold.session-board-reporter"
tail -n 100 "$HOME/Library/Logs/CommonKit/session-board-reporter.error.log"
```

## Add another machine

1. Generate a new reporter token. Never reuse another Target's token.
2. Add `"target-id":"new-token"` to `SESSION_BOARD_REPORTER_TOKENS` and rerun `install-vps.sh` with the complete token map, action token, and CF_Token. This refreshes the certificate as well as the service configuration.
3. On the new Mac, run `install-mac.sh` with the same Target id and token and a useful Target display name.
4. Open the board from a tailnet client and confirm the Target appears online. Stop the reporter and confirm it becomes offline before relying on the deployment.

## Rotate tokens

Reporter tokens are rotated one Target at a time:

1. Generate a new token and add it to the complete hub token map under the existing Target id.
2. Rerun `install-vps.sh`, then restart the hub if it is not already restarted by systemd.
3. Rerun `install-mac.sh` on that Mac with the new token.
4. Confirm the Target reconnects, then remove the old token if it temporarily used a separate Target id. The hub accepts one token per Target id, so the normal rotation briefly takes that reporter offline between steps 2 and 3.

To rotate the board action token, rerun `install-vps.sh` with the unchanged reporter map and a new `SESSION_BOARD_ACTION_TOKEN`, then replace the token used by the trusted board client. Existing board clients stop being able to submit decisions immediately. A failed or missing decision never becomes an approval; Claude returns to its normal TUI prompt on hook timeout or reporter failure.

Certificate renewal uses the same DNS-01 flow. Rerun `install-vps.sh` with `CF_Token` before expiry, or configure a reviewed local renewal job that runs `acme.sh --renew`, installs the renewed files at `/etc/caddy/certs/session-board/`, and reloads Caddy. Do not place the Cloudflare token in the hub environment file.
