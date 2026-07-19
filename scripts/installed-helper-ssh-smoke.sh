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
sudo usermod --password '*' "$user"
sudo chown -R "$user:$user" "$home" "$target" "$state"
host_key="$scratch/sshd/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$host_key"
cat > "$scratch/sshd/config" <<CFG
Port $port
ListenAddress 127.0.0.1
HostKey $host_key
PidFile $scratch/sshd/pid
AuthorizedKeysFile .ssh/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers $user
StrictModes yes
LogLevel ERROR
CFG
cleanup() {
  if [[ -f "$scratch/sshd/pid" ]]; then sudo kill "$(cat "$scratch/sshd/pid")" 2>/dev/null || true; fi
  sudo userdel "$user" 2>/dev/null || true
}
trap cleanup EXIT
sudo install -d -m 0755 /run/sshd
sudo /usr/sbin/sshd -f "$scratch/sshd/config"
known_hosts="$scratch/known_hosts"
ssh-keyscan -p "$port" 127.0.0.1 > "$known_hosts" 2>/dev/null
request='{"operation":"write_file","root_id":"home","path":"ssh-proof.txt","content":[115,115,104,10]}'
printf '%s' "$request" | ssh -T -i "$key" -p "$port" \
  -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
  -o "UserKnownHostsFile=$known_hosts" -- "$user@127.0.0.1" \
  "$home/commonkit-target-helper" --stdio-v1 > "$scratch/response.json"
node -e 'const r=require(process.argv[1]);if(r.result!=="applied")process.exit(1)' "$scratch/response.json"
test "$(cat "$target/ssh-proof.txt")" = ssh
