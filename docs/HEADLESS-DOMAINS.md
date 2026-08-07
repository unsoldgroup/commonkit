# Headless production domains

`commonkitd` loads optional production capability configuration from `headless.json` in CommonKit's private configuration directory. A missing file, or an omitted top-level capability, leaves only that capability unavailable. A present but malformed or incomplete capability fails daemon startup; CommonKit does not install an echo implementation or report fabricated success.

The daemon loads this file at process start. Desktop first-run onboarding writes it atomically and, when Desktop launched the bundled daemon, replaces only that owned process and waits for authenticated health before reporting success. If Desktop attached to a daemon managed by launchd, systemd, Windows Services, or another supervisor, it never terminates that process; onboarding reports that the external service manager must reload it instead of falsely claiming the new domains are active.

The configuration has four independent sections:

- `aboutMe` configures the encrypted owner profile database, its separately provisioned key reference, and the Loadout/project view exposed to agent tools. The database contains personal text and never enters Git. Agent tools can read the approved summary, search approved claims, submit direct-statement suggestions, and replace a conflicting claim only after explicit user clarification.

- `sync` identifies the target root, adapter state, declared and protected portable roots, composed-loadout and policy bindings, and a provider artifact store. Production loadouts use `providerPipeline`: CommonKit fetches and validates the trusted Git remote and exact pinned revision, runs configured native/APM/chezmoi providers in isolated controller workspaces, validates the combined ownership map, and persists digest-addressed `MaterializedState` records before planning. `materializedStates` remains a migration path and is mutually exclusive with `providerPipeline`. Provider code never runs during apply, recovery, or rollback.
- `sync.engram` enrols one opaque Engram chunk set with a portable project ID, owner principal, project root, chunk root, and `project` scope. Local targets require an explicit absolute, regular, non-symlink Engram executable. One target may declare `autoReconcile` with a configured peer and an interval of at least 60 seconds; the daemon serializes export, opaque chunk exchange, and import and writes a private movement receipt for every cycle. `syncTargets` declares additional local or SSH peers. Top-level `engramGrants` authorize cross-principal project transport; missing Grants, personal scope, invalid/missing Engram scope attestations, project mismatch, and chunk collisions fail closed. A withdrawn Grant stops future delivery but retains observations already imported.
- `credentials` maps stable destination IDs to a credential reference and a relative path below a capability-rooted private directory. Requests name destination IDs only. Secret bytes are resolved during confirmed apply; verification uses the applied receipt and does not resolve the provider again. Values never enter configuration responses, plans, receipts, logs, or portable metadata. The production registry accepts `env://` and absolute `file://` references, `bws://` references when an explicit BWS executable is configured, and `keychain://service/account` references through the target platform's native integration (`/usr/bin/security` on macOS, `/usr/bin/secret-tool` on Linux, and the native Credential Manager API on Windows). Unknown schemes, unavailable executables or stores, malformed references, symlinks in file references, and provider failures fail closed before credential mutation.

Relay reconciliation is a two-step review contract described by
`schemas/relay-reconcile.schema.json`. A proposal names only a configured
`targetId`, a future `confirmationId`, and an idempotency key. CommonKit loads
and integrity-checks that target's provider materialization, rejects capability
ownership collisions, and returns its recomputed declaration, provider-input,
ownership, artifact-set, and plan digests. Consent resubmits those exact digests
with `confirmed: true`. Caller-supplied resolved declarations or binding digests
are rejected. The confirmation ID is embedded in the immutable relay operation
payload and checked again by the durable executor, so a stale or replayed
confirmation cannot authorize a different reviewed state.
- `snapshots` maps database IDs to configured local database paths, source formats, and target identities. `portableState` is a required directory inside the Git kit: CommonKit atomically writes content-addressed snapshot descriptors there after uploading both the encrypted database and encrypted manifest to the object store. A second machine discovers the descriptor from Git and fetches the authenticated ciphertext; the separately provisioned key reference never enters Git. Use `sqlite` for SQLite databases so planning uses the online backup API and consumes WAL state consistently; `file` is reserved for stores whose own lifecycle guarantees a consistent single-file image. `observedPaths` explicitly maps any other locally inspectable target identity to its database path. Promotion hashes consistent exports of both the recorded writer and candidate itself; an unconfigured/unavailable target fails closed, and caller-provided digest assertions are rejected. The production `s3` backend uses the AWS CLI credential chain, keeping credentials outside CommonKit configuration; `local` is an explicit test/development backend. Authoritative-writer assignments are durable and list responses contain decrypted metadata only.

  Register the About Me database with `format: "file"`, not `format: "sqlite"`.
  Its SQLCipher file is already encrypted and the profile service uses short-lived
  connections without WAL mode, so the stopped file is the portable snapshot
  unit. The normal snapshot encryption is then applied around that ciphertext.
  Provision the same About Me key reference separately on each target; neither
  key enters the kit or snapshot metadata.

When `snapshots.gitAuthority` is configured, normal startup requires the append-only
`refs/commonkit-authority/<database>` chain ref on the trusted remote. Its commit ancestry
encodes every accepted authority generation. A missing ref is treated as rollback, including on
a fresh clone. The remote must protect `refs/commonkit-authority/*` from deletion and
non-fast-forward updates (for GitHub, install a repository ruleset covering that ref namespace and
disallow bypass). CommonKit atomically fast-forwards the kit branch and authority ref, but remote
protection is the durable trust boundary. The one exception is
explicit genesis: set `gitAuthority.bootstrap` to `true` only while CommonKit creates a
previously absent portable authority record. CommonKit publishes that record and its first
anchor atomically. Bootstrap is never applied to an existing record, so deleting every
anchor cannot silently reset trust; remove the flag after initialization as an operational
guardrail. Later unrelated fast-forward kit commits are valid because the authority chain tip
need only be an ancestor of the trusted branch head and the portable subtree must remain identical.

Example shape (digests abbreviated here must be full valid `sha256:` values in real configuration):

```json
{
  "aboutMe": {
    "database": "/home/al/.local/share/commonkit/about-me.sqlite",
    "keyReference": "file:///home/al/.config/commonkit/about-me.key",
    "loadoutId": "personal",
    "projectId": "commonkit",
    "agentId": "commonkit-agent"
  },
  "sync": {
    "targetId": "local",
    "targetRoot": "/home/al",
    "adapterState": "/var/lib/commonkit/filesystem",
    "providerArtifacts": "/var/lib/commonkit/provider-artifacts",
    "providerPipeline": {
      "root": "/var/lib/commonkit/provider-pipeline",
      "source": {
        "repository": "/home/al/.config/commonkit/kit",
        "trustedRemoteUrl": "git@github.com:example/commonkit-kit.git",
        "revision": "0123456789abcdef0123456789abcdef01234567"
      },
      "providers": [
        {
          "provider": "native",
          "version": "1.0.0",
          "files": [
            { "path": "home/.config/editor/config.json", "source": "portable/editor/config.json" }
          ]
        }
      ]
    },
    "declaredRoots": ["home"],
    "protectedRoots": ["home/.config/commonkit"],
    "caseSensitive": true,
    "targetIdentityDigest": "sha256:...",
    "composedLoadoutDigest": "sha256:...",
    "policyDigest": "sha256:...",
    "engram": {
      "projectId": "github.com/example/commonkit",
      "ownerId": "github:alice",
      "projectRoot": "repo",
      "root": "repo/.engram",
      "scope": "project",
      "executable": "/usr/local/bin/engram",
      "autoReconcile": {
        "peerTargetId": "workstation-b",
        "intervalSeconds": 60
      }
    }
  },
  "syncTargets": [
    {
      "targetId": "workstation-b",
      "targetRoot": "/home/alice",
      "adapterState": "/var/lib/commonkit/workstation-b-filesystem",
      "providerArtifacts": "/var/lib/commonkit/provider-artifacts",
      "materializedStates": ["/var/lib/commonkit/workstation-b-state.json"],
      "declaredRoots": ["repo"],
      "protectedRoots": [],
      "caseSensitive": true,
      "targetIdentityDigest": "sha256:...",
      "composedLoadoutDigest": "sha256:...",
      "policyDigest": "sha256:...",
      "engram": {
        "projectId": "github.com/example/commonkit",
        "ownerId": "github:alice",
        "projectRoot": "repo",
        "root": "repo/.engram",
        "scope": "project",
        "executable": "/usr/local/bin/engram"
      }
    }
  ],
  "credentials": {
    "root": "/home/al/.local/share/commonkit/credentials",
    "destinations": [
      { "id": "github-token", "reference": "env://GITHUB_TOKEN", "path": "github/token" }
    ]
  },
  "snapshots": {
    "root": "/home/al/.local/share/commonkit/snapshots",
    "portableState": "/home/al/.config/commonkit/kit/state",
    "keyReference": "file:///home/al/.config/commonkit/snapshot.key",
    "objectStore": {
      "type": "s3",
      "executable": "/usr/local/bin/aws",
      "endpoint": "https://s3.example.com",
      "bucket": "commonkit-snapshots",
      "prefix": "team/al"
    },
    "databases": [
      { "id": "context-mode", "path": "/home/al/.local/share/context-mode/context.sqlite", "targetId": "local", "format": "sqlite" }
    ]
  }
}
```

The file itself contains references and paths, never secret values. CommonKit's private-path enforcement remains responsible for its permissions.

For an SSH target, set `sync.targetTransport` to `type: "ssh"` with a stable root ID,
host, user, port, pinned `knownHosts` file, and SHA-256 host-key fingerprint. The daemon routes
plans containing `ssh-files` operations to a restart-safe typed SSH executor; provider code still
runs only in controller staging. The v1 remote helper can prove regular-file writes and removals,
so those resource types support inspect, plan, apply, verify, recovery, and rollback. Remote
directory and symlink intents fail closed until the helper protocol can inspect their type, mode,
and target without following links; CommonKit never approximates them as files or shell commands.
