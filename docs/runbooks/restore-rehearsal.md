# Restore and migration rehearsal

The installed rehearsal is `scripts/installed-lifecycle.sh`. It accepts only an
isolated scratch directory and a directory containing the installed
`commonkit`, `commonkitd`, `commonkit-target-helper`, and
`commonkit-snapshot-recovery-fixture` binaries:

```sh
scratch="$(mktemp -d /tmp/commonkit-installed.XXXXXX)"
scripts/installed-lifecycle.sh "$scratch" /path/to/installed/bin
```

The fixture keeps `HOME`, service state, credentials, provider artifacts,
portable state, snapshot objects, databases, and target roots below `scratch`.
It does not read or write a user's home directory or secret store. The checks
cover deterministic content-addressed snapshot IDs, repeat creation, exact
binary bytes (including NUL and `0xff`), empty-file restore, a clean plan with
zero operations, protected-root configuration, and recovery after a process is
killed immediately after the durable swap boundary.

The migration inventory must be exported as declarations before planning. Each
link is classified as a safe relative link, an absolute/traversal link to
rewrite or reject, a protected path, or an unsupported external dependency.
Only declarations under the isolated target root are eligible for a CommonKit
plan. A successful Linux rehearsal is evidence for Linux only; native macOS
execution is still required to accept the Mac link inventory, launchd/service
behavior, native keychain path, and signed install/update/uninstall lifecycle.
