# CommonKit release policy

Release automation runs only for version tags or an explicit workflow dispatch. It fails before packaging unless every platform signing identity, updater key, timestamp service, and update endpoint is configured. The checked-in workflow is executable, but must not be described as production-verified until it has run successfully with the production credentials.

## Release gate

A release requires a signed and notarized universal macOS application, a signed Windows NSIS installer, Linux AppImage and Debian packages, and a standalone CLI for every supported platform. Every payload is covered by the SHA-256 manifest and its GitHub OIDC signature, plus an SPDX SBOM and release notes. Tauri updater payloads carry dedicated detached updater signatures referenced by `latest.json`; `latest.json` is covered by the signed checksum manifest. Generated release notes are embedded in `latest.json` and shown before the separate, version-bound install confirmation.

The pipeline must fail closed when any code-signing, notarization, updater-signing, endpoint, checksum, signature, or SBOM input is absent or invalid. Secrets are supplied by the CI secret store and are never committed, logged, or replaced with development identities.

The separately dispatched lifecycle matrix first runs the desktop consent tests and a checked-in, non-production two-version updater fixture. The fixture verifies valid detached signatures and proves that tampering or replaying a newer signature against the older payload fails. The matrix then downloads two published releases, verifies their checksum manifests and GitHub OIDC identities, installs the older signed package, drives the authenticated Tauri updater to the newer package, verifies the installed desktop and managed daemon, runs the standalone CLI, and uninstalls the desktop package on macOS, Linux, and Windows.

The local fixture does not use a production updater identity. Production notarization and the end-to-end updater download remain release-candidate gates until the lifecycle workflow succeeds with production identities, two published versions, and the configured HTTPS endpoint. If releases remain in a private GitHub repository, that endpoint must be a separately hosted public feed; the unauthenticated desktop updater cannot consume private GitHub Release assets.

## Required CI configuration

Configure `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`, `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and `TAURI_UPDATER_PUBLIC_KEY` as repository secrets. Configure `WINDOWS_TIMESTAMP_URL` and `COMMONKIT_UPDATE_ENDPOINT` as repository variables; both must use HTTPS. Release asset URLs are derived from the exact repository and tag rather than supplied as mutable configuration.

## Third-party material

Generate an SBOM from the final artifact, not only source manifests. Review `THIRD-PARTY-NOTICES.md` against shipped binaries. If CommonKit begins redistributing chezmoi, APM, or another provider executable, include its complete upstream license notice in every affected distribution before enabling the release.
