#!/usr/bin/env bash
# Install commonkit-execd as a systemd user service on a Linux execution target.
#
# Run this ON the target, from a checkout of the revision to deploy. It is
# idempotent: re-running it rebuilds, reinstalls, and restarts.
#
# Secrets are read from the environment and written only to files this script
# creates with owner-only permissions. Nothing secret is echoed, and no secret
# is ever written to the repository.
#
#   COMMONKIT_EXECD_CLIENT_TOKEN
#   COMMONKIT_EXECD_WORKER_TOKEN
#   COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY
#   COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY
#   COMMONKIT_EXECD_WEBHOOK_SECRET   — enables the GitHub webhook route
#   GITHUB_STATUS_TOKEN              — enables commit statuses
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
config_root=${COMMONKIT_EXECD_CONFIG_ROOT:-/etc/commonkit}
state_root=${COMMONKIT_EXECD_STATE_ROOT:-/var/lib/commonkit-execd}
install_root=${COMMONKIT_EXECD_INSTALL_ROOT:-/usr/local/bin}
unit_root=${COMMONKIT_EXECD_UNIT_ROOT:-$HOME/.config/systemd/user}
worker_id=${COMMONKIT_EXECD_WORKER_ID:-linux-vps}

if [[ "$(uname -s)" != Linux ]]; then
  echo "commonkit-execd is deployed only to Linux targets" >&2
  exit 1
fi
for variable in COMMONKIT_EXECD_CLIENT_TOKEN COMMONKIT_EXECD_WORKER_TOKEN \
  COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY; do
  if [[ -z "${!variable:-}" ]]; then
    echo "$variable is required" >&2
    exit 1
  fi
done

# The target must be able to run what the loadout says it can run, or it would
# advertise a capability it cannot honour.
missing=()
for command in bwrap cargo git node pnpm rustc systemd-run; do
  command -v "$command" >/dev/null || missing+=("$command")
done
if ((${#missing[@]})); then
  echo "target is missing required commands: ${missing[*]}" >&2
  echo "see commonkit.ci-loadout.json" >&2
  exit 1
fi

echo "==> building commonkit-execd"
cargo build --release --locked -p commonkit-execd --bin commonkit-execd \
  --manifest-path "$repository_root/Cargo.toml"

echo "==> installing"
install -D -m 0755 "$repository_root/target/release/commonkit-execd" \
  "$install_root/commonkit-execd"
install -d -m 0755 "$config_root"
install -d -m 0700 "$state_root" "$state_root/workspaces" "$state_root/objects" \
  "$state_root/diagnostics"
install -m 0644 "$repository_root/commonkit.execution-target.example.json" \
  "$config_root/execution-target.json"
install -m 0644 "$repository_root/commonkit.execution-policy.example.json" \
  "$config_root/execution-policy.json"
install -m 0644 "$repository_root/commonkit.execution-context.json" \
  "$config_root/execution-context.json"

# Target-local secret values, keyed by the references a manifest may name. execd
# refuses this file unless it is owner-only.
umask 077
secrets=$(mktemp)
trap 'rm -f "$secrets"' EXIT
if [[ -n "${GITHUB_STATUS_TOKEN:-}" ]]; then
  GITHUB_STATUS_TOKEN="$GITHUB_STATUS_TOKEN" node -e \
    'process.stdout.write(JSON.stringify({"env://GITHUB_STATUS_TOKEN": process.env.GITHUB_STATUS_TOKEN}))' \
    >"$secrets"
else
  printf '{}' >"$secrets"
fi
install -m 0600 "$secrets" "$config_root/secrets.json"

environment=$(mktemp)
trap 'rm -f "$secrets" "$environment"' EXIT
{
  printf 'COMMONKIT_EXECD_CLIENT_TOKEN=%s\n' "$COMMONKIT_EXECD_CLIENT_TOKEN"
  printf 'COMMONKIT_EXECD_WORKER_TOKEN=%s\n' "$COMMONKIT_EXECD_WORKER_TOKEN"
  printf 'COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY=%s\n' "$COMMONKIT_EXECD_ARTIFACT_SIGNING_KEY"
  printf 'COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY=%s\n' "$COMMONKIT_EXECD_OBJECT_ENCRYPTION_KEY"
  if [[ -n "${COMMONKIT_EXECD_WEBHOOK_SECRET:-}" ]]; then
    printf 'COMMONKIT_EXECD_WEBHOOK_SECRET=%s\n' "$COMMONKIT_EXECD_WEBHOOK_SECRET"
  fi
} >"$environment"
install -m 0600 "$environment" "$config_root/execd.env"

echo "==> writing the unit"
install -d -m 0755 "$unit_root"
# Retaining a failed job's worktree is how an operator inspects what a check
# actually produced on the target; it is off unless asked for.
keep_failed=""
if [[ -n "${COMMONKIT_EXECD_KEEP_FAILED:-}" ]]; then
  keep_failed=" \\
  --keep-failed-workspaces"
fi
cat >"$unit_root/commonkit-execd.service" <<UNIT
[Unit]
Description=CommonKit durable execution daemon
After=network-online.target

[Service]
Type=simple
EnvironmentFile=$config_root/execd.env
WorkingDirectory=$state_root
ExecStart=$install_root/commonkit-execd \\
  --listen 127.0.0.1:7341 \\
  --database $state_root/jobs.db \\
  --worker-id $worker_id \\
  --target $config_root/execution-target.json \\
  --policy $config_root/execution-policy.json \\
  --secrets $config_root/secrets.json \\
  --tasks $config_root/execution-context.json \\
  --workspace-root $state_root/workspaces \\
  --object-root $state_root/objects \\
  --diagnostic-root $state_root/diagnostics$keep_failed
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
UNIT

systemctl --user daemon-reload
systemctl --user enable --now commonkit-execd.service
systemctl --user restart commonkit-execd.service
sleep 2
systemctl --user is-active commonkit-execd.service
echo "==> deployed"
