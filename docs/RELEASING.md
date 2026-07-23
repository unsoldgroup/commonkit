# CommonKit release policy

CommonKit does not use GitHub Actions. Releases are assembled from a trusted
release workstation and validated on manually invoked platform runners. No
GitHub-hosted workflow, check, billing state, or status is a release gate.

## Release gate

A release requires:

- a signed and notarized universal macOS application;
- a signed Windows NSIS installer;
- Linux AppImage and Debian packages;
- a standalone CLI for every supported platform;
- SHA-256 checksums, an SPDX SBOM, release notes, and detached Tauri updater
  signatures for every updater payload; and
- native install → reconcile → verify → snapshot/restore → recover → update →
  uninstall evidence from macOS, Linux, and Windows.

The release process fails closed when any code-signing, notarization,
updater-signing, endpoint, checksum, signature, SBOM, or native lifecycle input
is absent or invalid. Secrets stay in the operator's secret store and are
injected only into the release command; they are never committed or logged.

## Manual release procedure

1. Check out the exact version tag on the trusted release workstation.
2. Run the complete local test and lint commands from `CLAUDE.md`.
3. On each native platform or manually invoked platform runner, build the
   desktop and standalone CLI with the repository scripts:
   `scripts/build-release-cli.sh`, `scripts/stage-desktop-sidecars.sh`, and the
   Tauri build command.
4. Sign and notarize the native artifacts using the platform release identity.
5. Collect artifacts with `scripts/collect-release-assets.sh`.
6. Validate configuration with `scripts/prepare-release-config.mjs`.
7. Generate checksums with `scripts/checksum-release-assets.sh`, generate the
   updater manifest with `scripts/assemble-updater-manifest.mjs`, and validate
   the complete set with `scripts/verify-release-assets.mjs`.
8. Publish the two versioned release candidates to the chosen artifact store.
9. Set `COMMONKIT_QUALIFICATION_COMMIT` to the exact source commit and run
   `scripts/qualify-eight-flows.sh <evidence-root> <installed-bin>` on macOS,
   Linux, and Windows. It executes `scripts/installed-lifecycle.sh` in an
   isolated scratch directory and records the native platform, architecture,
   commit, result, and binary SHA-256 digests. Then run the signed two-version
   updater lifecycle on each platform.
10. Record the platform, architecture, artifact digests, commands, and results
    in the release evidence.

The local updater fixture does not use a production updater identity.
Production notarization and the end-to-end updater download remain
release-candidate gates until two published versions pass the native lifecycle.
If releases remain in a private repository, the desktop updater needs a
separately hosted public HTTPS feed.

## Required signing inputs

The trusted release environment must provide:

- `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`;
- `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD`, and an HTTPS Windows
  timestamp URL;
- `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and
  `TAURI_UPDATER_PUBLIC_KEY`; and
- an HTTPS `COMMONKIT_UPDATE_ENDPOINT`.

Release asset URLs are bound to the exact version and artifact digests rather
than mutable branch state.

## Third-party material

Generate an SBOM from final artifacts, not only source manifests. Review
`THIRD-PARTY-NOTICES.md` against shipped binaries. If CommonKit begins
redistributing chezmoi, APM, or another provider executable, include its
complete upstream license notice in every affected distribution.
