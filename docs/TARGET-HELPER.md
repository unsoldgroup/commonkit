# CommonKit target helper

`commonkit-target-helper` is the constrained mutation endpoint used for SSH targets. It accepts
only the versioned JSON protocol on standard input; it does not expose a command or shell surface.

## Safe bootstrap

1. Download the helper asset matching the target OS and CPU from the same CommonKit release as
   the controlling CLI. Never pipe a download into a shell.
2. Verify the release `SHA256SUMS` cosign bundle, then verify the helper against `SHA256SUMS`.
3. Copy the verified file over an already host-key-pinned SSH connection, install it as
   `commonkit-target-helper` in the remote account's fixed `PATH`, and make it executable only by
   that account. Do not grant sudo, setuid, or a login shell to the helper.
4. Create `~/.config/commonkit/target-helper.json` on the target with mode `0600`. Its
   `stateRoot` and every declared root must be absolute, pre-created directories; grant only the
   required `read_only` or `read_write` access. If this target participates in Engram chunk sync,
   set `engramExecutable` to an absolute, regular, non-symlink executable. The helper uses that
   fixed path only for typed export/import requests and never accepts a caller-supplied command.
5. Test the exact deployment using `commonkit targets verify <target>`. CommonKit invokes only
   `commonkit-target-helper --stdio-v1`, sends typed requests on stdin, pins the configured
   host-and-port key, and fails closed on missing or ambiguous pins.

Example configuration (paths are target-local):

```json
{
  "stateRoot": "/home/al/.local/state/commonkit/target-helper",
  "engramExecutable": "/home/al/.local/bin/engram",
  "roots": [
    { "id": "home", "path": "/home/al", "access": "read_write" }
  ]
}
```

The helper must be upgraded atomically with the controlling CommonKit release. Retain the prior
verified binary until the first post-upgrade target verification succeeds so rollback does not
depend on the network.
