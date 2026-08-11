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
   required `read_only` or `read_write` access.
5. Test the exact deployment using `commonkit targets verify <target>`. CommonKit invokes only
   `commonkit-target-helper --stdio-v1`, sends typed requests on stdin, pins the configured
   host-and-port key, and fails closed on missing or ambiguous pins.

The installer must write this file from a target probe; do not copy a hand-written
configuration or fabricate any digest. The following is the shape of an APT
configuration after probing (values are illustrative names only):

```json
{
  "stateRoot": "/home/al/.local/state/commonkit/target-helper",
  "roots": [
    { "id": "home", "path": "/home/al", "access": "read_write" }
  ],
  "packageResolution": {
    "target": {
      "os": "linux",
      "osVersion": "24.04",
      "distroId": "ubuntu",
      "distroVersion": "24.04",
      "codename": "noble",
      "arch": "amd64",
      "libc": "glibc"
    },
    "manager": {
      "manager": "apt",
      "version": "<probe output>",
      "executableDigest": "<probe digest>",
      "configDigest": "<probe digest>"
    },
    "policy": { "allowlists": {} },
    "apt": {
      "sourceId": "ubuntu-main",
      "suite": "noble",
      "components": ["main"],
      "signedBy": "/etc/commonkit/apt/archive-keyring.gpg",
      "signingAuthority": "ubuntu-key"
    },
    "targetIdentityDigest": "sha256:<64-hex>"
  }
}
```

`packageResolution` is optional and is disabled when absent. When present, its
manager binding, policy, source authority, and all resolver paths are target
local; the SSH request supplies only a typed desired package and a challenge.
The helper probes the configured target before resolving. A Node/NVM target
uses `node` instead of `apt` and keeps `nvmDir`, `shellExecutable`,
`releaseKeyring`, `gpgvExecutable`, and the probed
`gpgvExecutableDigest` in this target-only file. The helper rechecks that
executable (without following symlinks) immediately before signature
verification.

Provision the capability on the target with the installed helper, never by
editing digest fields by hand. The command is read-only except for the final
atomic config replacement and computes manager, gpgv, keyring, and executable
digests from the target it is running on:

```sh
commonkit-target-helper --probe-package-resolution \
  --config "$HOME/.config/commonkit/target-helper.json" \
  --manager nvm \
  --target-identity-digest 'sha256:<controller-bound identity>' \
  --nvm-dir "$HOME/.nvm" \
  --shell-executable /bin/bash \
  --release-keyring /etc/commonkit/node-release-keyring.kbx \
  --gpgv-executable /usr/bin/gpgv
```

APT provisioning takes `--apt-source-id`, `--apt-suite`,
`--apt-components`, `--apt-signed-by`, and `--apt-signing-authority`; these
are source policy inputs, not digest values. The command rejects symlinked or
group/world-writable config paths, writes mode `0600`, fsyncs, and validates
the resulting helper config before returning.

The helper must be upgraded atomically with the controlling CommonKit release. Retain the prior
verified binary until the first post-upgrade target verification succeeds so rollback does not
depend on the network.
