#!/usr/bin/env bash
set -euo pipefail

readonly DOMAIN="${LINEAR_GRAPH_DOMAIN:-graph.unsold.cloud}"
readonly HUB_PORT="${LINEAR_GRAPH_PORT:-8790}"
readonly TAILSCALE_IP="${LINEAR_GRAPH_TAILSCALE_IP:-100.71.109.66}"
readonly REPO_DIR="${LINEAR_GRAPH_REPO_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)}"
readonly TEMPLATE_DIR="$REPO_DIR/apps/linear-graph/deploy/templates"
readonly CONFIG_DIR="$HOME/.config/commonkit/linear-graph"
readonly SYSTEMD_DIR="$HOME/.config/systemd/user"
readonly ENV_FILE="$CONFIG_DIR/hub.env"
readonly UNIT_FILE="$SYSTEMD_DIR/linear-graph.service"
readonly CADDY_FILE="/etc/caddy/Caddyfile.d/linear-graph.caddy"
readonly CERT_DIR="/etc/caddy/certs/linear-graph"
readonly DATA_DIR="$HOME/.local/share/commonkit/linear-graph"
readonly CODEX_AUTH="${LINEAR_GRAPH_CODEX_AUTH:-subscription}"
readonly CODEX_HOME="${LINEAR_GRAPH_CODEX_HOME:-$DATA_DIR/codex}"

require_command() { command -v "$1" >/dev/null 2>&1 || { printf 'Required command not found: %s\n' "$1" >&2; exit 1; }; }
systemd_quote() {
  local value=$1
  [[ $value != *$'\n'* && $value != *$'\r'* ]] || { printf 'Environment values must not contain newlines.\n' >&2; exit 1; }
  value=${value//\\/\\\\}; value=${value//\"/\\\"}; printf '"%s"' "$value"
}
render_service() {
  sed -e "s|@@REPO_DIR@@|${REPO_DIR//&/\\&}|g" -e "s|@@BUN_BIN@@|${BUN_BIN//&/\\&}|g" -e "s|@@DATA_DIR@@|${DATA_DIR//&/\\&}|g" -e "s|@@CODEX_HOME@@|${CODEX_HOME//&/\\&}|g" "$TEMPLATE_DIR/linear-graph.service" >"$UNIT_FILE"
}

for command_name in bun pnpm systemctl sudo caddy; do require_command "$command_name"; done
BUN_BIN="$(command -v bun)"
readonly BUN_BIN
readonly ACME_SH="${ACME_SH:-$HOME/.acme.sh/acme.sh}"
[[ -x "$ACME_SH" ]] || { printf 'acme.sh not found at %s\n' "$ACME_SH" >&2; exit 1; }
: "${LINEAR_API_TOKEN:?Set LINEAR_API_TOKEN}"
: "${LINEAR_GRAPH_ACTION_TOKEN:?Set LINEAR_GRAPH_ACTION_TOKEN}"
case "$CODEX_AUTH" in
  subscription) ;;
  api-key) : "${CODEX_API_KEY:?Set CODEX_API_KEY when LINEAR_GRAPH_CODEX_AUTH=api-key}" ;;
  *) printf 'LINEAR_GRAPH_CODEX_AUTH must be subscription or api-key.\n' >&2; exit 1 ;;
esac
if [[ -z ${CF_Token:-} && -r ${HOME}/.acme.sh/account.conf ]]; then
  # Reuse the credential acme.sh already stores on this VPS without copying it into our env file.
  # shellcheck disable=SC1091
  source "${HOME}/.acme.sh/account.conf"
  export CF_Token="${SAVED_CF_Token:-}"
fi
: "${CF_Token:?Set CF_Token for the Cloudflare DNS challenge}"

mkdir -p "$CONFIG_DIR" "$SYSTEMD_DIR" "$DATA_DIR" "$CODEX_HOME"
if [[ "$CODEX_HOME" == "$DATA_DIR/codex" && -f "$HOME/.codex/auth.json" && ! -f "$CODEX_HOME/auth.json" ]]; then
  install -m 600 "$HOME/.codex/auth.json" "$CODEX_HOME/auth.json"
fi
umask 077
chmod 700 "$CONFIG_DIR" "$DATA_DIR"
{
  printf 'LINEAR_API_TOKEN=%s\n' "$(systemd_quote "$LINEAR_API_TOKEN")"
  [[ "$CODEX_AUTH" != "api-key" ]] || printf 'CODEX_API_KEY=%s\n' "$(systemd_quote "$CODEX_API_KEY")"
  printf 'LINEAR_GRAPH_ACTION_TOKEN=%s\n' "$(systemd_quote "$LINEAR_GRAPH_ACTION_TOKEN")"
  printf 'LINEAR_GRAPH_HOST=%s\n' "$(systemd_quote 127.0.0.1)"
  printf 'LINEAR_GRAPH_PORT=%s\n' "$(systemd_quote "$HUB_PORT")"
  printf 'LINEAR_GRAPH_DATA_PATH=%s\n' "$(systemd_quote "$DATA_DIR/graph.sqlite")"
  [[ -z ${LINEAR_GRAPH_TIMEZONE:-} ]] || printf 'LINEAR_GRAPH_TIMEZONE=%s\n' "$(systemd_quote "$LINEAR_GRAPH_TIMEZONE")"
  printf 'CODEX_HOME=%s\n' "$(systemd_quote "$CODEX_HOME")"
  printf 'LINEAR_GRAPH_CODEX_AUTH=%s\n' "$(systemd_quote "$CODEX_AUTH")"
  [[ -z ${LINEAR_GRAPH_CODEX_MODEL:-} ]] || printf 'LINEAR_GRAPH_CODEX_MODEL=%s\n' "$(systemd_quote "$LINEAR_GRAPH_CODEX_MODEL")"
  [[ -z ${LINEAR_GRAPH_REPO_MAP:-} ]] || printf 'LINEAR_GRAPH_REPO_MAP=%s\n' "$(systemd_quote "$LINEAR_GRAPH_REPO_MAP")"
  [[ -z ${LINEAR_GRAPH_EXECUTION_REPOS:-} ]] || printf 'LINEAR_GRAPH_EXECUTION_REPOS=%s\n' "$(systemd_quote "$LINEAR_GRAPH_EXECUTION_REPOS")"
} >"$ENV_FILE"
chmod 600 "$ENV_FILE"

render_service
chmod 644 "$UNIT_FILE"
pnpm --dir "$REPO_DIR" install --frozen-lockfile
pnpm --dir "$REPO_DIR" --filter @commonkit/linear-graph-protocol build
pnpm --dir "$REPO_DIR" --filter @commonkit/linear-graph-web build

export CF_Token
"$ACME_SH" --issue --dns dns_cf --keylength ec-256 -d "$DOMAIN"
CERT_STAGE="$(mktemp -d)"; trap 'rm -rf -- "$CERT_STAGE"' EXIT
"$ACME_SH" --install-cert -d "$DOMAIN" --ecc --key-file "$CERT_STAGE/key.pem" --fullchain-file "$CERT_STAGE/fullchain.pem"
sudo install -d -m 0750 -o root -g caddy "$CERT_DIR" /etc/caddy/Caddyfile.d
sudo install -m 0640 -o root -g caddy "$CERT_STAGE/key.pem" "$CERT_DIR/key.pem"
sudo install -m 0644 -o root -g caddy "$CERT_STAGE/fullchain.pem" "$CERT_DIR/fullchain.pem"
sed -e "s|@@DOMAIN@@|$DOMAIN|g" -e "s|@@HUB_PORT@@|$HUB_PORT|g" -e "s|@@TAILSCALE_IP@@|$TAILSCALE_IP|g" "$TEMPLATE_DIR/linear-graph.caddy" | sudo tee "$CADDY_FILE" >/dev/null
sudo caddy validate --config /etc/caddy/Caddyfile
systemctl --user daemon-reload
systemctl --user enable --now linear-graph.service
sudo systemctl reload caddy

printf 'Linear graph installed at https://%s (bound to %s only)\n' "$DOMAIN" "$TAILSCALE_IP"
printf 'Create a DNS-only A record for %s pointing to %s; this script does not change DNS.\n' "$DOMAIN" "$TAILSCALE_IP"
