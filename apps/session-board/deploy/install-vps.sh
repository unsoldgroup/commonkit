#!/usr/bin/env bash
set -euo pipefail

readonly DOMAIN="${SESSION_BOARD_DOMAIN:-board.unsold.cloud}"
readonly HUB_PORT="${SESSION_BOARD_PORT:-8787}"
readonly REPO_DIR="${SESSION_BOARD_REPO_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../../.." && pwd)}"
readonly TEMPLATE_DIR="$REPO_DIR/apps/session-board/deploy/templates"
readonly CONFIG_DIR="$HOME/.config/commonkit/session-board"
readonly SYSTEMD_DIR="$HOME/.config/systemd/user"
readonly ENV_FILE="$CONFIG_DIR/hub.env"
readonly UNIT_FILE="$SYSTEMD_DIR/board-hub.service"
readonly CADDY_FILE="/etc/caddy/Caddyfile.d/session-board.caddy"
readonly CERT_DIR="/etc/caddy/certs/session-board"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    printf 'Required command not found: %s\n' "$1" >&2
    exit 1
  }
}

systemd_quote() {
  local value=$1
  [[ $value != *$'\n'* && $value != *$'\r'* ]] || {
    printf 'Environment values must not contain newlines.\n' >&2
    exit 1
  }
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  printf '"%s"' "$value"
}

render_template() {
  local source=$1 destination=$2 repo_escaped bun_escaped port_escaped
  repo_escaped=${REPO_DIR//&/\\&}
  bun_escaped=${BUN_BIN//&/\\&}
  port_escaped=${HUB_PORT//&/\\&}
  sed -e "s|@@REPO_DIR@@|$repo_escaped|g" \
    -e "s|@@BUN_BIN@@|$bun_escaped|g" \
    -e "s|@@HUB_PORT@@|$port_escaped|g" \
    "$source" >"$destination"
}

require_command bun
require_command pnpm
require_command systemctl
require_command sudo
require_command caddy

BUN_BIN="$(command -v bun)"
readonly BUN_BIN
readonly ACME_SH="${ACME_SH:-$HOME/.acme.sh/acme.sh}"

[[ -x $ACME_SH ]] || {
  printf 'acme.sh not found at %s. Install it first; see docs/session-board.md.\n' "$ACME_SH" >&2
  exit 1
}
: "${SESSION_BOARD_REPORTER_TOKENS:?Set SESSION_BOARD_REPORTER_TOKENS to a JSON object of Target ids to tokens}"
: "${SESSION_BOARD_ACTION_TOKEN:?Set SESSION_BOARD_ACTION_TOKEN}"
: "${HOSTINGER_API_TOKEN:?Set HOSTINGER_API_TOKEN for the acme.sh Hostinger DNS challenge}"

mkdir -p "$CONFIG_DIR" "$SYSTEMD_DIR" "$REPO_DIR/apps/session-board/hub/data"
umask 077
{
  printf 'SESSION_BOARD_REPORTER_TOKENS=%s\n' "$(systemd_quote "$SESSION_BOARD_REPORTER_TOKENS")"
  printf 'SESSION_BOARD_ACTION_TOKEN=%s\n' "$(systemd_quote "$SESSION_BOARD_ACTION_TOKEN")"
  printf 'SESSION_BOARD_HOST=%s\n' "$(systemd_quote '127.0.0.1')"
  printf 'SESSION_BOARD_PORT=%s\n' "$(systemd_quote "$HUB_PORT")"
} >"$ENV_FILE"
chmod 600 "$ENV_FILE"

render_template "$TEMPLATE_DIR/board-hub.service" "$UNIT_FILE"
chmod 644 "$UNIT_FILE"

pnpm --dir "$REPO_DIR" install --frozen-lockfile
pnpm --dir "$REPO_DIR" --filter @commonkit/session-board-web build

export HOSTINGER_API_TOKEN
"$ACME_SH" --issue --dns dns_hostinger --keylength ec-256 -d "$DOMAIN"
CERT_STAGE="$(mktemp -d)"
readonly CERT_STAGE
trap 'rm -rf -- "$CERT_STAGE"' EXIT
"$ACME_SH" --install-cert -d "$DOMAIN" --ecc \
  --key-file "$CERT_STAGE/key.pem" \
  --fullchain-file "$CERT_STAGE/fullchain.pem"

sudo install -d -m 0750 -o root -g caddy "$CERT_DIR" /etc/caddy/Caddyfile.d
sudo install -m 0640 -o root -g caddy "$CERT_STAGE/key.pem" "$CERT_DIR/key.pem"
sudo install -m 0644 -o root -g caddy "$CERT_STAGE/fullchain.pem" "$CERT_DIR/fullchain.pem"
sed -e "s|@@DOMAIN@@|$DOMAIN|g" -e "s|@@HUB_PORT@@|$HUB_PORT|g" \
  "$TEMPLATE_DIR/session-board.caddy" | sudo tee "$CADDY_FILE" >/dev/null

sudo caddy validate --config /etc/caddy/Caddyfile
systemctl --user daemon-reload
systemctl --user enable --now board-hub.service
sudo systemctl reload caddy

printf 'Session Board hub installed at https://%s\n' "$DOMAIN"
printf 'DNS is not changed by this script. Create the tailnet-only A record described in docs/session-board.md.\n'
