#!/usr/bin/env bash
# Provision the two host-level prerequisites commonkit-execd cannot install for
# itself, on a Linux execution target. Run this ON the target.
#
#   credentials  a git credential store, so the mirror of a private repository
#                can be fetched without a token ever appearing in a manifest
#   dns          the public A record the webhook endpoint answers on
#
# Both read their token from Bitwarden Secrets Manager at run time. No secret is
# printed, and none is written anywhere except the owner-only credential file.
#
#   ./provision-execd-host.sh credentials
#   ./provision-execd-host.sh dns
#   ./provision-execd-host.sh all
set -euo pipefail

host_ip=${COMMONKIT_EXECD_PUBLIC_IP:-72.60.44.157}
host_name=${COMMONKIT_EXECD_PUBLIC_NAME:-exec}
zone_name=${COMMONKIT_EXECD_ZONE:-unsold.cloud}

secret() {
  bws secret list | jq -r --arg key "$1" '.[] | select(.key==$key) | .value' | head -1
}

provision_credentials() {
  local token
  token=$(secret GITHUB_TOKEN)
  if [[ -z "$token" ]]; then
    echo "GITHUB_TOKEN is not in Bitwarden Secrets Manager" >&2
    exit 1
  fi
  umask 077
  printf 'https://x-access-token:%s@github.com\n' "$token" >"$HOME/.git-credentials"
  chmod 600 "$HOME/.git-credentials"
  git config --global credential.helper store
  # Prove the credential works before a job depends on it.
  git ls-remote --heads https://github.com/unsoldgroup/commonkit.git main >/dev/null
  echo "credentials: private repository reachable"
}

provision_dns() {
  local token zone existing
  token=$(secret cloudflare/SECURITY_REMEDIATION_TOKEN)
  if [[ -z "$token" ]]; then
    echo "cloudflare/SECURITY_REMEDIATION_TOKEN is not in Bitwarden Secrets Manager" >&2
    exit 1
  fi
  zone=$(curl -sS -H "Authorization: Bearer $token" \
    "https://api.cloudflare.com/client/v4/zones?name=$zone_name" | jq -r '.result[0].id')
  if [[ -z "$zone" || "$zone" == null ]]; then
    echo "zone $zone_name not found" >&2
    exit 1
  fi
  existing=$(curl -sS -H "Authorization: Bearer $token" \
    "https://api.cloudflare.com/client/v4/zones/$zone/dns_records?name=$host_name.$zone_name" |
    jq -r '.result[0].id // empty')
  local body
  body=$(jq -nc --arg name "$host_name" --arg content "$host_ip" \
    '{type:"A",name:$name,content:$content,ttl:300,proxied:false,comment:"CommonKit execution API (USG-51)"}')
  if [[ -n "$existing" ]]; then
    curl -sS -X PUT -H "Authorization: Bearer $token" -H "Content-Type: application/json" \
      --data "$body" \
      "https://api.cloudflare.com/client/v4/zones/$zone/dns_records/$existing" |
      jq -c '{updated:.success,name:.result.name,content:.result.content}'
  else
    curl -sS -X POST -H "Authorization: Bearer $token" -H "Content-Type: application/json" \
      --data "$body" \
      "https://api.cloudflare.com/client/v4/zones/$zone/dns_records" |
      jq -c '{created:.success,name:.result.name,content:.result.content}'
  fi
}

provision_tls() {
  local fqdn="$host_name.$zone_name"
  local cert_root=${COMMONKIT_EXECD_CERT_ROOT:-/etc/caddy/certs/execution-api}
  local vhost=${COMMONKIT_EXECD_VHOST:-/etc/caddy/Caddyfile.d/execution-api.caddy}
  local token
  token=$(secret cloudflare/SECURITY_REMEDIATION_TOKEN)
  if [[ -z "$token" ]]; then
    echo "cloudflare/SECURITY_REMEDIATION_TOKEN is not in Bitwarden Secrets Manager" >&2
    exit 1
  fi
  install -d -m 0755 "$cert_root"
  # DNS-01, because the zone is served by Cloudflare and the endpoint answers on
  # a port the challenge does not need.
  CF_Token="$token" "$HOME/.acme.sh/acme.sh" --issue --dns dns_cf -d "$fqdn" \
    --home "$HOME/.acme.sh" --keylength ec-256 || [[ $? == 2 ]] # 2 = already valid
  # Caddy runs as its own user, so it must be able to read the key it serves —
  # group-readable, never world-readable.
  "$HOME/.acme.sh/acme.sh" --install-cert -d "$fqdn" --ecc --home "$HOME/.acme.sh" \
    --fullchain-file "$cert_root/fullchain.pem" \
    --key-file "$cert_root/key.pem" \
    --reloadcmd "chown root:caddy $cert_root/fullchain.pem $cert_root/key.pem && chmod 640 $cert_root/key.pem && chmod 644 $cert_root/fullchain.pem && systemctl reload caddy"
  chown root:caddy "$cert_root/fullchain.pem" "$cert_root/key.pem"
  chmod 640 "$cert_root/key.pem"
  chmod 644 "$cert_root/fullchain.pem"
  cat >"$vhost" <<VHOST
https://$fqdn {
	tls $cert_root/fullchain.pem $cert_root/key.pem
	reverse_proxy 127.0.0.1:7341
}
VHOST
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
  systemctl reload caddy
  echo "tls: https://$fqdn is served"
}

# Open 443 only to the source ranges GitHub actually delivers webhooks from.
# The endpoint authenticates every request, but an execution daemon has no
# reason to present its TLS stack to the whole internet when one caller needs it.
provision_firewall() {
  local ranges
  ranges=$(curl -sS https://api.github.com/meta | jq -r '.hooks[]')
  if [[ -z "$ranges" ]]; then
    echo "could not read GitHub's webhook source ranges" >&2
    exit 1
  fi
  # Drop rules for ranges GitHub no longer publishes, so the allowlist tracks
  # the source of truth instead of only ever growing.
  while read -r rule; do
    ufw --force delete "$rule" >/dev/null || true
  done < <(ufw status numbered | grep -n '443/tcp' | grep -oP '(?<=\[)\s*\d+(?=\])' | tr -d ' ' | sort -rn)
  local count=0
  while read -r range; do
    [[ -z "$range" ]] && continue
    ufw allow from "$range" to any port 443 proto tcp comment "GitHub webhooks (USG-51)" >/dev/null
    count=$((count + 1))
  done <<<"$ranges"
  echo "firewall: 443 open to $count GitHub webhook ranges"
}

case "${1:?usage: provision-execd-host.sh credentials|dns|tls|firewall|all}" in
credentials) provision_credentials ;;
dns) provision_dns ;;
tls) provision_tls ;;
firewall) provision_firewall ;;
all)
  provision_credentials
  provision_dns
  provision_tls
  provision_firewall
  ;;
*)
  echo "unknown step: $1" >&2
  exit 1
  ;;
esac
