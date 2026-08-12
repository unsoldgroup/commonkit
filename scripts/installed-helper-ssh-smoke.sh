#!/usr/bin/env bash
# Linux CI-only proof that the installed helper works through a real, isolated sshd.
set -euo pipefail

scratch=${1:?usage: installed-helper-ssh-smoke.sh SCRATCH INSTALLED_BIN}
installed_bin=${2:?usage: installed-helper-ssh-smoke.sh SCRATCH INSTALLED_BIN}
helper="$installed_bin/commonkit-target-helper"
test -x "$helper"

port=40222
user=commonkit_smoke
# GitHub's RUNNER_TEMP ancestors are intentionally not traversable by another
# account. Keep keys and logs there, but place the simulated remote target under
# a unique private root owned by the privilege-dropped account. The helper's
# capability-safe artifact store must be able to read every state-path component.
runtime_root=$(mktemp -d /tmp/commonkit-ssh-smoke.XXXXXX)
home="$runtime_root/home"
target="$runtime_root/target"
state="$runtime_root/state"
key="$scratch/id_ed25519"
mkdir -p "$home/.ssh" "$home/.config/commonkit" "$target" "$state" "$scratch/sshd"
ssh-keygen -q -t ed25519 -N '' -f "$key"
cp "$key.pub" "$home/.ssh/authorized_keys"
cp "$helper" "$home/commonkit-target-helper"
cat > "$home/.config/commonkit/target-helper.json" <<JSON
{"stateRoot":"$state","roots":[{"id":"home","path":"$target","access":"read_write"}]}
JSON
chmod 0700 "$home" "$home/.ssh"
chmod 0600 "$home/.ssh/authorized_keys" "$home/.config/commonkit/target-helper.json"
chmod 0755 "$home/commonkit-target-helper"
sudo useradd --home-dir "$home" --no-create-home --shell /bin/sh "$user"
# OpenSSH rejects locked or expired accounts before considering authorized_keys.
# Give this disposable account a valid, random shadow hash and clear its account
# expiry. The isolated daemon below still requires public-key authentication, so
# the generated password is never an accepted authentication method.
password_hash=$(openssl rand -hex 32 | openssl passwd -6 -stdin)
printf '%s:%s\n' "$user" "$password_hash" | sudo chpasswd --encrypted
unset password_hash
sudo chage --expiredate -1 "$user"
sudo awk -F: -v user="$user" '$1 == user && $2 ~ /^\$6\$/ && $8 == "" { found = 1 } END { exit !found }' /etc/shadow
sudo chown -R "$user:$user" "$home" "$target" "$state"
sudo chown "$user:$user" "$runtime_root"
sudo chmod 0700 "$runtime_root"
# These assertions reproduce the hosted failure before sshd obscures it as a
# generic public-key rejection, and ensure no broader access is required.
sudo -u "$user" test -r "$runtime_root" -a -x "$runtime_root"
sudo -u "$user" test -r "$home/.ssh/authorized_keys"
sudo -u "$user" test -x "$home/commonkit-target-helper"
sudo -u "$user" test -w "$target" -a -w "$state"
cleanup_runtime() {
  if [[ -f "$scratch/sshd/pid" ]]; then
    sudo kill "$(cat "$scratch/sshd/pid")" 2>/dev/null || true
  fi
  sudo userdel "$user" 2>/dev/null || true
  rm -f "$home/.config/commonkit/.target-helper."*.tmp "$scratch/sshd/pid"
  case "$runtime_root" in
    /tmp/commonkit-ssh-smoke.*) sudo rm -rf -- "$runtime_root" ;;
    *) printf 'refusing to remove unexpected runtime root: %s\n' "$runtime_root" >&2 ;;
  esac
}
trap cleanup_runtime EXIT INT TERM
if [[ "${COMMONKIT_PACKAGE_SUPPORT:-0}" == "1" && "${COMMONKIT_ENABLE_PACKAGE_PROBE:-0}" != "1" ]]; then
  echo 'package support requires an explicit successful native probe' >&2
  exit 1
fi
if [[ "${COMMONKIT_ENABLE_PACKAGE_PROBE:-0}" == "1" ]]; then
  : "${COMMONKIT_TARGET_IDENTITY_DIGEST:?set a controller-bound target identity digest}"
  : "${COMMONKIT_PACKAGE_MANAGER:?set apt or nvm}"
  probe_args=(
    --probe-package-resolution
    --config "$home/.config/commonkit/target-helper.json"
    --manager "$COMMONKIT_PACKAGE_MANAGER"
    --target-identity-digest "$COMMONKIT_TARGET_IDENTITY_DIGEST"
  )
  if [[ "$COMMONKIT_PACKAGE_MANAGER" == apt ]]; then
    probe_args+=(
      --apt-source-id "${COMMONKIT_APT_SOURCE_ID:?}"
      --apt-suite "${COMMONKIT_APT_SUITE:?}"
      --apt-components "${COMMONKIT_APT_COMPONENTS:?}"
      --apt-signed-by "${COMMONKIT_APT_SIGNED_BY:?}"
      --apt-signing-authority "${COMMONKIT_APT_SIGNING_AUTHORITY:?}"
      --apt-metadata-digest "${COMMONKIT_APT_METADATA_DIGEST:?set the trusted signed-metadata sha256 digest}"
    )
  else
    probe_args+=(
      --nvm-dir "${COMMONKIT_NVM_DIR:?}"
      --shell-executable "${COMMONKIT_SHELL_EXECUTABLE:?}"
      --release-keyring "${COMMONKIT_RELEASE_KEYRING:?}"
      --gpgv-executable "${COMMONKIT_GPGV_EXECUTABLE:?}"
    )
  fi
  sudo -u "$user" "$home/commonkit-target-helper" "${probe_args[@]}"
fi
host_key="$scratch/sshd/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$host_key"
cat > "$scratch/sshd/config" <<CFG
Port $port
ListenAddress 127.0.0.1
HostKey $host_key
PidFile $scratch/sshd/pid
AuthorizedKeysFile .ssh/authorized_keys
PubkeyAuthentication yes
AuthenticationMethods publickey
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers $user
StrictModes yes
LogLevel VERBOSE
CFG
sshd_log="$scratch/sshd/sshd.log"
cleanup_sshd() {
  if [[ -f "$scratch/sshd/pid" ]]; then sudo kill "$(cat "$scratch/sshd/pid")" 2>/dev/null || true; fi
  rm -f "$home/.config/commonkit/.target-helper."*.tmp "$scratch/sshd/pid"
}
trap 'cleanup_sshd; cleanup_runtime' EXIT
sudo install -d -m 0755 /run/sshd
sudo /usr/sbin/sshd -t -f "$scratch/sshd/config"
sudo /usr/sbin/sshd -E "$sshd_log" -f "$scratch/sshd/config"
known_hosts="$scratch/known_hosts"
ssh-keyscan -p "$port" 127.0.0.1 > "$known_hosts" 2>/dev/null
request='{"operation":"write_file","root_id":"home","path":"ssh-proof.txt","content":[115,115,104,10]}'
redact_diagnostics() {
  sed -E \
    -e 's#(/tmp/commonkit-ssh-smoke\.)[^ /]+#\1[redacted]#g' \
    -e 's#SHA256:[A-Za-z0-9+/=]+#SHA256:[redacted]#g'
}
# Prove the installed helper, config, root, and request agree before adding the
# SSH transport boundary. This catches capability/path regressions directly.
direct_response="$scratch/direct-response.json"
direct_stderr="$scratch/direct-helper-stderr.log"
if ! {
  printf '%s' "$request" | sudo -u "$user" env -i HOME="$home" PATH=/usr/bin:/bin \
    "$home/commonkit-target-helper" --stdio-v1
} > "$direct_response" 2> "$direct_stderr"; then
  sed -n '1,40p' "$direct_stderr" | redact_diagnostics >&2
  exit 1
fi
node -e 'const r=require(process.argv[1]);if(r.result!=="applied")process.exit(1)' "$direct_response"
test "$(sudo -u "$user" cat "$target/ssh-proof.txt")" = ssh
sudo -u "$user" rm "$target/ssh-proof.txt"
helper_stderr="$scratch/helper-stderr.log"
if ! printf '%s' "$request" | ssh -T -i "$key" -p "$port" \
  -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
  -o "UserKnownHostsFile=$known_hosts" -- "$user@127.0.0.1" \
  "$home/commonkit-target-helper" --stdio-v1 > "$scratch/response.json" 2> "$helper_stderr"; then
  sed -n '1,40p' "$helper_stderr" | redact_diagnostics >&2
  sudo sed -n '1,120p' "$sshd_log" | redact_diagnostics >&2
  exit 1
fi
node -e 'const r=require(process.argv[1]);if(r.result!=="applied")process.exit(1)' "$scratch/response.json"
test "$(sudo -u "$user" cat "$target/ssh-proof.txt")" = ssh
