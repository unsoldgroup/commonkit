# Headless production domains

`commonkitd` loads optional production capability configuration from `headless.json` in CommonKit's private configuration directory. A missing file, or an omitted top-level capability, leaves only that capability unavailable. A present but malformed or incomplete capability fails daemon startup; CommonKit does not install an echo implementation or report fabricated success.

The configuration has three independent sections:

- `sync` identifies the target root, adapter state, declared and protected portable roots, composed-loadout and policy bindings, a provider artifact store, and one or more durable `MaterializedState` files. Provider resolution does not run inside a plan, verify, apply, recovery, or rollback request. Planning validates provider provenance, ownership, artifacts, and all plan bindings before persisting the content-addressed plan.
- `credentials` maps stable destination IDs to a credential reference and a relative path below a capability-rooted private directory. Requests name destination IDs only. Secret bytes are resolved at apply or verify time and never enter configuration responses, plans, receipts, logs, or portable metadata. The production registry currently accepts local `env://` and `file://` references; other schemes fail closed until their resolver is explicitly wired.
- `snapshots` maps database IDs to configured local database paths, source formats, and target identities. Use `sqlite` for SQLite databases so planning uses the online backup API and consumes WAL state consistently; `file` is reserved for stores whose own lifecycle guarantees a consistent single-file image. Both manifests and objects are authenticated and encrypted with a target-local key reference. The production `s3` backend uses the AWS CLI credential chain, keeping credentials outside CommonKit configuration; `local` is an explicit test/development backend. Authoritative-writer assignments are durable and list responses contain decrypted metadata only.

Example shape (digests abbreviated here must be full valid `sha256:` values in real configuration):

```json
{
  "sync": {
    "targetId": "local",
    "targetRoot": "/home/al",
    "adapterState": "/var/lib/commonkit/filesystem",
    "providerArtifacts": "/var/lib/commonkit/provider-artifacts",
    "materializedStates": ["/var/lib/commonkit/providers/native.json"],
    "declaredRoots": ["home"],
    "protectedRoots": ["home/.config/commonkit"],
    "caseSensitive": true,
    "targetIdentityDigest": "sha256:...",
    "composedLoadoutDigest": "sha256:...",
    "observedDigest": "sha256:...",
    "policyDigest": "sha256:..."
  },
  "credentials": {
    "root": "/home/al/.local/share/commonkit/credentials",
    "destinations": [
      { "id": "github-token", "reference": "env://GITHUB_TOKEN", "path": "github/token" }
    ]
  },
  "snapshots": {
    "root": "/home/al/.local/share/commonkit/snapshots",
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
