#!/usr/bin/env bash
# End-to-end CI proof using only executables installed into the test prefix.
set -euo pipefail

scratch=${1:?usage: installed-lifecycle.sh SCRATCH}
installed_bin=${2:?usage: installed-lifecycle.sh SCRATCH INSTALLED_BIN}
installed_bin=$(cd "$installed_bin" && pwd -P)
real_git=$(command -v git)
mkdir -p "$scratch/source/layers" "$scratch/source/portable" "$scratch/source/targets" \
  "$scratch/tools" "$scratch/target" "$scratch/secondary-target"

commonkit="$installed_bin/commonkit"
commonkitd="$installed_bin/commonkitd"
target_helper="$installed_bin/commonkit-target-helper"
recovery_fixture="$installed_bin/commonkit-snapshot-recovery-fixture"
if [[ "${RUNNER_OS:-}" == Windows ]]; then
  commonkit="$commonkit.exe"
  commonkitd="$commonkitd.exe"
  target_helper="$target_helper.exe"
  recovery_fixture="$recovery_fixture.exe"
fi
for executable in "$commonkit" "$commonkitd" "$target_helper" "$recovery_fixture"; do test -x "$executable"; done

# Keep every installed smoke artifact and service definition inside the runner scratch root.
export HOME="$scratch/home"
export USERPROFILE="$scratch/home"
export XDG_CONFIG_HOME="$scratch/home/.config"
export XDG_DATA_HOME="$scratch/home/.local/share"
export XDG_CACHE_HOME="$scratch/home/.cache"
export COMMONKIT_SERVICE_BACKEND=process_fallback
export COMMONKIT_SERVICE_ROOT="$scratch/service"
export COMMONKIT_RELAY_PORT=0
mkdir -p "$HOME"

printf 'installed lifecycle fixture\n' > "$scratch/source/portable/editor.conf"
node - "$scratch/source/layers" <<'JS'
const { createHash } = require("node:crypto");
const { writeFileSync } = require("node:fs");
const { join } = require("node:path");

const layersRoot = process.argv[2];
const canonical = (value) => {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    const entries = Object.keys(value).sort().map(
      (key) => `${JSON.stringify(key)}:${canonical(value[key])}`,
    );
    return `{${entries.join(",")}}`;
  }
  return JSON.stringify(value);
};
const writeLayer = (name, kind, spec) => {
  const layer = {
    schemaVersion: 1,
    id: name,
    kind,
    source: {
      path: `layers/${name}.json`,
      revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    },
    spec,
  };
  const digest = createHash("sha256")
    .update("commonkit.layer-content.v1")
    .update(Buffer.from([0]))
    .update(canonical(layer))
    .digest("hex");
  layer.source.contentDigest = `sha256:${digest}`;
  writeFileSync(join(layersRoot, `${name}.json`), `${JSON.stringify(layer)}\n`);
};

writeLayer("public-base", "public_base", {});
writeLayer("organization-policy", "organization_policy", {});
writeLayer("personal", "personal_kit", {
  files: [{ path: "portable/editor.conf", source: "portable/editor.conf" }],
});
writeLayer("project-web", "project_loadout", {});
writeLayer("target-local", "target_overrides", {});
JS
printf '%s\n' '{"schemaVersion":1,"id":"secondary","loadout":"personal","projectLoadout":"project-web","targetOverride":"target-local","transport":"local","managedBy":"commonkit"}' \
  > "$scratch/source/targets/secondary.json"
git -C "$scratch/source" init --quiet
git -C "$scratch/source" config user.email commonkit-ci@example.invalid
git -C "$scratch/source" config user.name CommonKit-CI
git -C "$scratch/source" remote add origin https://github.com/owner/installed-fixture.git
git -C "$scratch/source" add .
git -C "$scratch/source" commit --quiet -m fixture

cat > "$scratch/tools/gh.mjs" <<'JS'
import { cpSync } from "node:fs";
const args = process.argv.slice(2);
if (args[0] === "auth" && args[1] === "status") process.exit(0);
if (args[0] === "repo" && args[1] === "clone" && args[3]) {
  cpSync(process.env.COMMONKIT_FIXTURE_REPO, args[3], { recursive: true });
  process.exit(0);
}
process.exit(91);
JS
cat > "$scratch/tools/gh" <<'SH'
#!/usr/bin/env bash
exec node "$COMMONKIT_FIXTURE_TOOLS/gh.mjs" "$@"
SH
chmod +x "$scratch/tools/gh"
cat > "$scratch/tools/gh.cmd" <<'CMD'
@node "%COMMONKIT_FIXTURE_TOOLS%\gh.mjs" %*
CMD
cat > "$scratch/tools/git-proxy.mjs" <<'JS'
import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
appendFileSync(process.env.COMMONKIT_GIT_HELPER_LOG, `${JSON.stringify(args)}\n`);
const commandIndex = args[0] === "-C" ? 2 : 0;
const command = args[commandIndex];
const tail = args.slice(commandIndex + 1);
if (command === "fetch") process.exit(0);
if (command === "push") process.exit(0);
if (command === "rev-parse" && tail.some((value) => value.startsWith("refs/remotes/origin/"))) {
  const result = spawnSync(process.env.COMMONKIT_REAL_GIT, [...args.slice(0, commandIndex), "rev-parse", "HEAD"], { stdio: "inherit" });
  process.exit(result.status ?? 1);
}
if (command === "rev-list" && tail.includes("--left-right")) {
  process.stdout.write("0\t0\n");
  process.exit(0);
}
const result = spawnSync(process.env.COMMONKIT_REAL_GIT, args, { stdio: "inherit" });
process.exit(result.status ?? 1);
JS
cat > "$scratch/tools/git" <<'SH'
#!/usr/bin/env bash
exec node "$COMMONKIT_FIXTURE_TOOLS/git-proxy.mjs" "$@"
SH
chmod +x "$scratch/tools/git"
cat > "$scratch/tools/git.cmd" <<'CMD'
@node "%COMMONKIT_FIXTURE_TOOLS%\git-proxy.mjs" %*
CMD
cat > "$scratch/tools/bws-fixture" <<'SH'
#!/usr/bin/env bash
[[ "$1" == secret && "$2" == get && "$3" == installed-secret-id ]] || exit 7
printf '%s' '{"value":"installed credential proof"}'
SH
chmod +x "$scratch/tools/bws-fixture"
cat > "$scratch/tools/bws-fixture.cmd" <<'CMD'
@if not "%1 %2 %3"=="secret get installed-secret-id" exit /b 7
@echo {"value":"installed credential proof"}
CMD
bws_fixture="$scratch/tools/bws-fixture"
if [[ "${RUNNER_OS:-}" == Windows ]]; then bws_fixture="$scratch/tools/bws-fixture.cmd"; fi
cat > "$scratch/tools/read-control-status.mjs" <<'JS'
import { readFileSync, writeFileSync } from "node:fs";
const [discoveryPath, tokenPath, outputPath] = process.argv.slice(2);
const { port } = JSON.parse(readFileSync(discoveryPath, "utf8"));
const token = readFileSync(tokenPath, "utf8").trim();
const response = await fetch(`http://127.0.0.1:${port}/control/v1/status`, {
  headers: { authorization: `Bearer ${token}` },
});
if (!response.ok) process.exit(1);
writeFileSync(outputPath, await response.text());
JS
export COMMONKIT_FIXTURE_REPO="$scratch/source"
export COMMONKIT_FIXTURE_TOOLS="$scratch/tools"
export COMMONKIT_REAL_GIT="$real_git"
export COMMONKIT_GIT_HELPER_LOG="$scratch/git-helper.log"
export PATH="$scratch/tools:$installed_bin:$PATH"

stop_daemon() {
  "$commonkit" daemon uninstall >/dev/null 2>&1 || true
}
start_daemon() {
  "$commonkit" daemon install >/dev/null
  if ! "$commonkit" daemon start >/dev/null; then
    cat "$scratch/service/commonkitd.log" >&2 || true
    return 1
  fi
  for _ in $(seq 1 150); do
    if "$commonkit" relay status >/dev/null 2>&1; then return; fi
    sleep 0.1
  done
  cat "$scratch/service/commonkitd.log" >&2
  return 1
}
cleanup_probe_temps() {
  rm -f "$HOME/.config/commonkit/.target-helper."*.tmp
}
trap 'stop_daemon; cleanup_probe_temps' EXIT
start_daemon
"$commonkit" daemon status | node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>{const s=JSON.parse(b);if(!s.installed||!s.running)process.exit(1)})'
"$commonkit" daemon restart >/dev/null
for _ in $(seq 1 150); do
  if "$commonkit" relay status >/dev/null 2>&1; then restarted=1; break; fi
  sleep 0.1
done
test "${restarted:-}" = 1

# Exercise the installed headless profile path. The answer file is local
# transient input; only its encrypted revision may remain in CommonKit state.
profile_secret='installed profile plaintext sentinel'
printf '%s\n' "{\"identity.display_name\":\"$profile_secret\"}" > "$scratch/profile-answers.json"
"$commonkit" profile schema > "$scratch/profile-schema.json"
node -e 'const s=require(process.argv[1]);if(Object.keys(s.fields).length!==38)process.exit(1)' "$scratch/profile-schema.json"
"$commonkit" profile encrypt \
  --answers "$scratch/profile-answers.json" \
  --profile-id installed-profile \
  --revision-id installed-revision \
  --recipient age1hhdujv6pev30q36uwkqnhep7z6sgzlevx4jtj3hqwpfwkq853yesd4sd3x \
  --recipient age1eccdk92w98zscva4q372yh4phdl33zl0r5zawhns4hyuug37wfssz6dxea \
  --recipient age1wdf4dx4artmfxhwhm626qjtsw66spue3yrf59vusvc3q0gnyj59sgp9vs0 \
  --confirmed > "$scratch/profile-encryption.json"
node - "$scratch/profile-encryption.json" "$XDG_DATA_HOME/state/personal-context/staged/installed-revision.json" "$profile_secret" <<'JS'
const fs = require("node:fs");
const [receiptPath, revisionPath, secret] = process.argv.slice(2);
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
if (!receipt.staged || receipt.revisionId !== "installed-revision" ||
    !receipt.ciphertextDigest?.startsWith("sha256:")) process.exit(1);
if (fs.readFileSync(receiptPath, "utf8").includes(secret) ||
    fs.readFileSync(revisionPath, "utf8").includes(secret)) process.exit(1);
JS
node -e 'require("node:fs").unlinkSync(process.argv[1])' "$scratch/profile-answers.json"

init_json="$scratch/init.json"
"$commonkit" init connect \
  --repository owner/installed-fixture \
  --kit-directory "$scratch/kit" \
  --loadout personal \
  --project-loadout project-web \
  --target-override target-local \
  --target local \
  --publish-registration \
  --target-root "$scratch/target" > "$init_json"
headless=$(node -e 'const fs=require("fs");process.stdout.write(JSON.parse(fs.readFileSync(process.argv[1])).headlessConfig)' "$init_json")

# Add a second target-specific runtime domain from the same portable five-layer
# loadout. The provider still computes desired state; each filesystem adapter
# owns only its declared live target root.
node - "$headless" "$scratch/secondary-target" "$scratch" <<'JS'
const { createHash } = require("node:crypto");
const fs = require("node:fs");
const [configPath, secondaryRoot, scratch] = process.argv.slice(2);
const config = JSON.parse(fs.readFileSync(configPath));
config.sync.protectedRoots = ["portable/.commonkit"];
const secondary = structuredClone(config.sync);
secondary.targetId = "secondary";
secondary.targetRoot = secondaryRoot;
secondary.adapterState = `${scratch}/secondary-state/filesystem`;
secondary.providerArtifacts = `${scratch}/secondary-state/provider-artifacts`;
secondary.providerPipeline.root = `${scratch}/secondary-state/provider-pipeline`;
secondary.targetIdentityDigest = `sha256:${createHash("sha256").update("installed-secondary-target").digest("hex")}`;
config.syncTargets = [secondary];
fs.writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`);
JS

snapshot_root="$scratch/snapshots"
database="$snapshot_root/database.bin"
empty_database="$snapshot_root/empty.bin"
key_file="$scratch/snapshot.key"
mkdir -p "$snapshot_root"
printf 'database before snapshot' > "$database"
touch "$empty_database"
node -e 'require("fs").writeFileSync(process.argv[1], Buffer.alloc(32,47))' "$key_file"

# Keep snapshot lifecycle callbacks independent from the daemon under test. The
# callback is an installed-safe target-local command, so restore cannot deadlock
# by recursively calling the daemon that is serving the restore request.
cat > "$scratch/tools/snapshot-lifecycle" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod +x "$scratch/tools/snapshot-lifecycle"
cat > "$scratch/tools/snapshot-lifecycle.cmd" <<'CMD'
@exit /b 0
CMD
snapshot_lifecycle="$scratch/tools/snapshot-lifecycle"
if [[ "${RUNNER_OS:-}" == Windows ]]; then snapshot_lifecycle="$scratch/tools/snapshot-lifecycle.cmd"; fi

# First adopt the clean, pinned provider configuration generated by onboarding.
"$commonkit" daemon reload-domains --confirmed >/dev/null

"$commonkit" status >/dev/null
"$commonkit" compose > "$scratch/composition.json"
node -e 'const c=require(process.argv[1]);if(c.lock.layers.length!==5)process.exit(1)' "$scratch/composition.json"
"$commonkit" targets select local secondary --confirmed > "$scratch/target-selection.json"
"$commonkit" targets list | node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>{const s=JSON.parse(b);if(s.targets.length!==2||s.selected.length!==2)process.exit(1)})'
plan_json="$scratch/plan.json"
"$commonkit" sync --confirmed > "$plan_json"
plan_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));process.stdout.write(p.planId ?? p.id)' "$plan_json")
"$commonkit" diff "$plan_id" >/dev/null
"$commonkit" apply "$plan_id" --confirmed > "$scratch/apply.json"
apply_run_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));if(!p.runId)process.exit(1);process.stdout.write(p.runId)' "$scratch/apply.json")
for _ in $(seq 1 150); do
  if "$commonkit" verify >/dev/null 2>&1 &&
    cmp -s "$scratch/source/portable/editor.conf" "$scratch/target/portable/editor.conf"; then
    verified=1
    break
  fi
  sleep 0.1
done
test "${verified:-}" = 1
cmp "$scratch/source/portable/editor.conf" "$scratch/target/portable/editor.conf"

# A repeated plan over the verified target is explicit about being clean.
"$commonkit" sync --confirmed > "$scratch/clean-plan.json"
node - "$scratch/clean-plan.json" <<'JS'
const fs = require("node:fs");
const plan = JSON.parse(fs.readFileSync(process.argv[2]));
if (!Array.isArray(plan.operations) || plan.operations.length !== 0) process.exit(1);
JS

# Plan, apply, and verify both independently addressed target domains through
# the installed multi-target CLI surface.
"$commonkit" targets plan local secondary --confirmed > "$scratch/target-plans.json"
secondary_plan_id=$(node -e 'const p=require(process.argv[1]);if(p.length!==2||p.some(x=>!(x.planId??x.id)?.startsWith("sha256:")))process.exit(1);process.stdout.write(p[1].planId??p[1].id)' "$scratch/target-plans.json")
"$commonkit" targets apply secondary "$secondary_plan_id" --confirmed > "$scratch/secondary-apply.json"
"$commonkit" targets verify local secondary > "$scratch/target-verification.json"
cmp "$scratch/source/portable/editor.conf" "$scratch/secondary-target/portable/editor.conf"
"$commonkit" targets select secondary --confirmed > "$scratch/scheduler-target-selection.json"

# Exercise explicit receipt-bound rollback through the installed CLI and daemon
# and prove the exact absent preimage was restored. A rolled-back plan remains
# idempotently bound to its original confirmation and is not silently re-run.
"$commonkit" rollback "$apply_run_id" --plan-id "$plan_id" --confirmed > "$scratch/rollback.json"
node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));if(p.outcome!=="rolledback"||p.runId!==process.argv[2])process.exit(1)' "$scratch/rollback.json" "$apply_run_id"
test ! -e "$scratch/target/portable/editor.conf"

# The same authenticated atomic reload used by Desktop must adopt mutable-state
# configuration without replacing either an owned or service-manager-owned process.
# Snapshot authority bootstrap intentionally writes portable Git state, so initialize
# it only after the provider-backed plan has completed against its pinned clean revision.
node - "$headless" "$snapshot_root" "$database" "$key_file" "$snapshot_lifecycle" "$scratch/kit" "$scratch/credentials" "$bws_fixture" <<'JS'
const fs = require("fs");
const [configPath, root, database, key, lifecycleExecutable, portableState, credentialRoot, bwsExecutable] = process.argv.slice(2);
const config = JSON.parse(fs.readFileSync(configPath));
const lifecycle = { executable: lifecycleExecutable, args: [] };
config.credentials = {
  root: credentialRoot,
  bwsExecutable,
  destinations: [{ id: "api-token", reference: "bws://installed-secret-id", path: "tokens/api" }],
};
config.snapshots = {
  root,
  portableState,
  keyReference: `file://${key}`,
  objectStore: { type: "local" },
  databases: [{ id: "context-mode", path: database, targetId: "local", format: "file",
    lifecycle: { stop: lifecycle, start: lifecycle } }],
};
fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
JS
"$commonkit" daemon reload-domains --confirmed >/dev/null

# Credential provisioning must traverse the installed CLI and daemon's redacted
# plan/apply/verify transaction rather than writing directly to the destination.
credential_plan_json="$scratch/credential-plan.json"
"$commonkit" credentials plan api-token > "$credential_plan_json"
credential_plan_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));if(!p.planId?.startsWith("sha256:")||p.operations?.[0]?.destinationId!=="api-token"||JSON.stringify(p).includes("installed credential proof"))process.exit(1);process.stdout.write(p.planId)' "$credential_plan_json")
"$commonkit" credentials apply --plan-id "$credential_plan_id" --confirmed > "$scratch/credential-apply.json"
"$commonkit" credentials verify api-token > "$scratch/credential-verify.json"
test "$(cat "$scratch/credentials/tokens/api")" = 'installed credential proof'
node -e 'const fs=require("fs");for(const p of process.argv.slice(1)){const v=JSON.parse(fs.readFileSync(p));if(JSON.stringify(v).includes("installed credential proof"))process.exit(1)}' "$scratch/credential-plan.json" "$scratch/credential-apply.json" "$scratch/credential-verify.json"

# Enabling the installed scheduler must wake the already-running daemon, perform
# a read-only drift tick, persist its state, and stop ticking after disable.
printf 'external scheduled drift\n' > "$scratch/secondary-target/portable/editor.conf"
scheduled_target_digest=$(node -e 'const fs=require("fs"),c=require("crypto");process.stdout.write(c.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"))' "$scratch/secondary-target/portable/editor.conf")
"$commonkit" schedule enable --interval-seconds 1 --confirmed > "$scratch/schedule-enable.json"
for _ in $(seq 1 50); do
  node "$scratch/tools/read-control-status.mjs" "$XDG_DATA_HOME/state/daemon.json" "$XDG_CONFIG_HOME/control.token" "$scratch/scheduled-status.json"
  if node -e 'const s=require(process.argv[1]);process.exit(s.lastDriftCheckUnixMs == null||s.lastDriftErrorCode!=="selected_target_drifted" ? 1 : 0)' "$scratch/scheduled-status.json"; then scheduled_tick=1; break; fi
  sleep 0.1
done
test "${scheduled_tick:-}" = 1
test "$(node -e 'const fs=require("fs"),c=require("crypto");process.stdout.write(c.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"))' "$scratch/secondary-target/portable/editor.conf")" = "$scheduled_target_digest"
scheduled_tick_time=$(node -e 'process.stdout.write(String(require(process.argv[1]).lastDriftCheckUnixMs))' "$scratch/scheduled-status.json")
"$commonkit" schedule disable --confirmed > "$scratch/schedule-disable.json"
"$commonkit" schedule status | node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>{const s=JSON.parse(b);if(s.enabled)process.exit(1)})'
sleep 1.2
node "$scratch/tools/read-control-status.mjs" "$XDG_DATA_HOME/state/daemon.json" "$XDG_CONFIG_HOME/control.token" "$scratch/disabled-schedule-status.json"
test "$(node -e 'process.stdout.write(String(require(process.argv[1]).lastDriftCheckUnixMs))' "$scratch/disabled-schedule-status.json")" = "$scheduled_tick_time"

"$commonkit" snapshots list | node -e '
let body="";
process.stdin.on("data", chunk => body += chunk);
process.stdin.on("end", () => {
  const state=JSON.parse(body);
  if (state.snapshots.length !== 0 || state.writers["context-mode"] !== "local" ||
      state.authority["context-mode"].pendingInitialization !== true) process.exit(1);
});'

snapshot_json="$scratch/snapshot.json"
"$commonkit" snapshots create context-mode --confirmed > "$snapshot_json"
snapshot_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));if(!p.snapshotId?.startsWith("snapshot-"))process.exit(1);process.stdout.write(p.snapshotId)' "$snapshot_json")
"$commonkit" snapshots create context-mode --confirmed > "$scratch/snapshot-repeat.json"
node -e 'const fs=require("fs");const first=JSON.parse(fs.readFileSync(process.argv[1]));const second=JSON.parse(fs.readFileSync(process.argv[2]));if(first.snapshotId!==second.snapshotId)process.exit(1)' "$snapshot_json" "$scratch/snapshot-repeat.json"
printf 'database changed after snapshot' > "$database"
"$commonkit" snapshots restore "$snapshot_id" --confirmed >/dev/null
test "$(cat "$database")" = 'database before snapshot'

# File snapshots preserve arbitrary bytes, including NULs and 0xff, and an empty file.
node - "$database" <<'JS'
const fs = require("node:fs");
fs.writeFileSync(process.argv[2], Buffer.from([0, 255, 1, 2, 0, 255]));
JS
"$commonkit" snapshots create context-mode --confirmed > "$scratch/binary-snapshot.json"
binary_snapshot_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));if(!p.snapshotId?.startsWith("snapshot-"))process.exit(1);process.stdout.write(p.snapshotId)' "$scratch/binary-snapshot.json")
printf 'database changed after binary snapshot' > "$database"
"$commonkit" snapshots restore "$binary_snapshot_id" --confirmed >/dev/null
node - "$database" <<'JS'
const fs = require("node:fs");
const expected = Buffer.from([0, 255, 1, 2, 0, 255]);
if (!fs.readFileSync(process.argv[2]).equals(expected)) process.exit(1);
JS
node - "$empty_database" <<'JS'
const fs = require("node:fs");
if (fs.statSync(process.argv[2]).size !== 0) process.exit(1);
JS

# A second isolated database fixture proves the same exact-byte contract for empty files.
node - "$database" "$empty_database" <<'JS'
const fs = require("node:fs");
fs.copyFileSync(process.argv[3], process.argv[2]);
JS
"$commonkit" snapshots create context-mode --confirmed > "$scratch/empty-snapshot.json"
empty_snapshot_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));if(!p.snapshotId?.startsWith("snapshot-"))process.exit(1);process.stdout.write(p.snapshotId)' "$scratch/empty-snapshot.json")
printf 'non-empty after empty snapshot' > "$database"
"$commonkit" snapshots restore "$empty_snapshot_id" --confirmed >/dev/null
test ! -s "$database"

# The installed helper stops after the atomic swap. A new daemon process discovers the
# authenticated transaction and rolls it back without re-running a provider or secret lookup.
stop_daemon
"$recovery_fixture" interrupt "$snapshot_root"
test "$(cat "$database")" = 'staged restored database'
start_daemon
for _ in $(seq 1 100); do
  [[ "$(cat "$database")" == 'pre-interruption database' ]] && break
  sleep 0.1
done
test "$(cat "$database")" = 'pre-interruption database'
stop_daemon
"$recovery_fixture" recover "$snapshot_root"

# A real process kill after the durable swap boundary must be recoverable by a fresh daemon.
if [[ "${RUNNER_OS:-}" != Windows ]]; then
  "$recovery_fixture" sigkill "$snapshot_root" >/dev/null 2>&1 || test "$?" -eq 137
  test "$(cat "$database")" = 'staged restored database'
  start_daemon
  for _ in $(seq 1 100); do
    [[ "$(cat "$database")" == 'pre-interruption database' ]] && break
    sleep 0.1
  done
  test "$(cat "$database")" = 'pre-interruption database'
  stop_daemon
fi

# Exercise the actual installed, typed stdin-only ssh remote helper protocol.
helper_target="$scratch/helper-target"
helper_state="$scratch/helper-state"
mkdir -p "$HOME/.config/commonkit" "$helper_target"
node - "$HOME/.config/commonkit/target-helper.json" "$helper_state" "$helper_target" <<'JS'
const fs = require("fs");
fs.writeFileSync(process.argv[2], JSON.stringify({
  stateRoot: process.argv[3],
  roots: [{ id: "home", path: process.argv[4], access: "read_write" }],
}));
JS
chmod 0600 "$HOME/.config/commonkit/target-helper.json"
if [[ "${COMMONKIT_PACKAGE_SUPPORT:-0}" == "1" && "${COMMONKIT_ENABLE_PACKAGE_PROBE:-0}" != "1" ]]; then
  echo 'package support requires an explicit successful native probe' >&2
  exit 1
fi
if [[ "${COMMONKIT_ENABLE_PACKAGE_PROBE:-0}" == "1" ]]; then
  : "${COMMONKIT_TARGET_IDENTITY_DIGEST:?set a controller-bound target identity digest}"
  : "${COMMONKIT_PACKAGE_MANAGER:?set apt or nvm}"
  probe_args=(
    --probe-package-resolution
    --config "$HOME/.config/commonkit/target-helper.json"
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
    )
  else
    probe_args+=(
      --nvm-dir "${COMMONKIT_NVM_DIR:?}"
      --shell-executable "${COMMONKIT_SHELL_EXECUTABLE:?}"
      --release-keyring "${COMMONKIT_RELEASE_KEYRING:?}"
      --gpgv-executable "${COMMONKIT_GPGV_EXECUTABLE:?}"
    )
  fi
  "$target_helper" "${probe_args[@]}"
fi
printf '%s' '{"operation":"write_file","root_id":"home","path":"portable/helper-proof.txt","content":[104,101,108,112,101,114,10]}' |
  "$target_helper" --stdio-v1 > "$scratch/helper-response.json"
node -e 'const r=require(process.argv[1]);if(r.result!=="applied")process.exit(1)' "$scratch/helper-response.json"
test "$(cat "$helper_target/portable/helper-proof.txt")" = helper

stop_daemon
"$commonkit" daemon status | node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>{const s=JSON.parse(b);if(s.installed||s.running)process.exit(1)})'
