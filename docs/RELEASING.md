# CommonKit release policy

Release automation runs only for version tags or an explicit workflow dispatch. It fails before packaging unless every platform signing identity, updater key, timestamp service, and update endpoint is configured. The checked-in workflow is executable, but must not be described as production-verified until it has run successfully with the production credentials.

## Release gate

A release requires a signed and notarized universal macOS application, a signed Windows NSIS installer, Linux AppImage and Debian packages, and a standalone CLI for every supported platform. Every payload is covered by the SHA-256 manifest and its GitHub OIDC signature, plus an SPDX SBOM and release notes. Tauri updater payloads carry dedicated detached updater signatures referenced by `latest.json`; `latest.json` is covered by the signed checksum manifest.

The pipeline must fail closed when any code-signing, notarization, updater-signing, endpoint, checksum, signature, or SBOM input is absent or invalid. Secrets are supplied by the CI secret store and are never committed, logged, or replaced with development identities.

The separately dispatched lifecycle matrix downloads two published releases, verifies their checksum manifests and GitHub OIDC identities, installs the older signed package, installs the newer signed package as an upgrade, runs the standalone CLI, and uninstalls the desktop package on macOS, Linux, and Windows. This proves installer lifecycle behavior; it does not yet automate the in-application updater UI or a deliberately invalid updater signature. Those remain release-candidate gates and must not be inferred from a green packaging job.

## Third-party material

Generate an SBOM from the final artifact, not only source manifests. Review `THIRD-PARTY-NOTICES.md` against shipped binaries. If CommonKit begins redistributing chezmoi, APM, or another provider executable, include its complete upstream license notice in every affected distribution before enabling the release.
