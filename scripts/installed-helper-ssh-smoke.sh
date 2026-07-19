#!/usr/bin/env bash
# Linux CI-only proof that the installed helper works through a real, isolated sshd.
set -euo pipefail

scratch=${1:?usage: installed-helper-ssh-smoke.sh SCRATCH INSTALLED_BIN}
installed_bin=${2:?usage: installed-helper-ssh-smoke.sh SCRATCH INSTALLED_BIN}
helper="$installed_bin/commonkit-target-helper"
test -x "$helper"

port=40222
user=commonkit_smoke
home="$scratch/home"
target="$scratch/target"
state="$scratch/state"
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
cleanup() {
  if [[ -f "$scratch/sshd/pid" ]]; then sudo kill "$(cat "$scratch/sshd/pid")" 2>/dev/null || true; fi
  sudo userdel "$user" 2>/dev/null || true
}
trap cleanup EXIT
sudo install -d -m 0755 /run/sshd
sudo /usr/sbin/sshd -t -f "$scratch/sshd/config"
sudo /usr/sbin/sshd -E "$sshd_log" -f "$scratch/sshd/config"
known_hosts="$scratch/known_hosts"
ssh-keyscan -p "$port" 127.0.0.1 > "$known_hosts" 2>/dev/null
request='{"operation":"write_file","root_id":"home","path":"ssh-proof.txt","content":[115,115,104,10]}'
if ! printf '%s' "$request" | ssh -T -i "$key" -p "$port" \
  -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
  -o "UserKnownHostsFile=$known_hosts" -- "$user@127.0.0.1" \
  "$home/commonkit-target-helper" --stdio-v1 > "$scratch/response.json"; then
  sudo sed -n '1,120p' "$sshd_log" >&2
  exit 1
fi
node -e 'const r=require(process.argv[1]);if(r.result!=="applied")process.exit(1)' "$scratch/response.json"
test "$(cat "$target/ssh-proof.txt")" = ssh
