# CommonKit release policy

Release automation is intentionally disabled until production signing identities and real installer lifecycle fixtures are provisioned. The checked-in workflows validate the contract only; they must not be described as release-ready.

## Release gate

A release requires a signed and notarized universal macOS application, a signed Windows NSIS installer, Linux AppImage and Debian packages, and a standalone CLI for every supported platform. Every payload ships with SHA-256 checksums, a detached signature, an SPDX SBOM, release notes, and schema compatibility metadata. Tauri updater artifacts and `latest.json` are signed by the dedicated updater key.

The pipeline must fail closed when any code-signing, notarization, updater-signing, endpoint, checksum, signature, or SBOM input is absent or invalid. Secrets are supplied by the CI secret store and are never committed, logged, or replaced with development identities.

Before enabling packaging, the install/update/uninstall matrix must use two real signed versions on macOS, Linux, and Windows. It must verify application and standalone CLI launch, update-signature rejection, preserved user data, removal of services and startup entries, and the documented explicit option for deleting CommonKit state.

## Third-party material

Generate an SBOM from the final artifact, not only source manifests. Review `THIRD-PARTY-NOTICES.md` against shipped binaries. If CommonKit begins redistributing chezmoi, APM, or another provider executable, include its complete upstream license notice in every affected distribution before enabling the release.
