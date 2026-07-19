# Unattended daemon service

An installed CommonKit CLI manages its matching `commonkitd` as a per-user service:

```console
commonkit daemon install
commonkit daemon start
commonkit daemon status
commonkit daemon restart
commonkit daemon uninstall
```

The CLI requires `commonkitd` beside itself, writes a fixed executable field without a shell, and
rejects symlinked executables, service roots, and definitions. macOS uses a LaunchAgent, Linux uses
the systemd user manager, and Windows uses a least-privilege per-user Scheduled Task. Installation
does not grant administrator privileges.

On Linux hosts without a usable systemd user manager, CommonKit reports and uses the explicit
`process_fallback` backend. That backend keeps the daemon running unattended for the current login
session, but cannot promise restart after logout or reboot; invoke `commonkit daemon start` from the
host's user-session startup facility or install a supported user service manager. `status` always
reports the selected backend, installation state, running state, and definition path.

Uninstall stops the managed process and removes CommonKit's service definition and PID metadata.
Portable kit data, receipts, snapshots, credentials, and provider inputs are deliberately retained.
