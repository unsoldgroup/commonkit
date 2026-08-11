# Package resolution threat model

## Boundary

A desired-state provider may declare an exact package and typed selector. It cannot submit a repository binding, resolution document, artifact reference, fetch request, or install recipe.

The trusted controller owns `PackageResolutionCoordinator`. The coordinator validates the package-source allowlist, target tuple, manager executable and configuration digests, and CommonKit source-registry entry before any backend or fetch call.

Only a registered `PackageResolutionBackend` receives `PackageFetch`. Package planners and offline adapters receive only a persisted `ResolvedPackageIntent`.

## Required bindings

`PackageResolutionV1` binds these values before plan approval:

- the exact declaration and selector;
- the target operating system, distribution, architecture, libc, and manager prefix;
- the manager identity, version, executable digest, and configuration digest;
- the source-registry definition, canonical repository, immutable revision, and authenticated metadata;
- the before observation, dependency closure, artifacts, and offline install recipe.

The coordinator verifies every fetched artifact's digest and size before it stores the artifact. The persisted resolution must list the exact canonical artifact references. Missing, extra, duplicate, conflicting, or corrupt artifacts fail closed.

## Recovery and restart

Planning, apply preparation, verification, and restart recovery load only the persisted resolution and artifacts. They do not invoke a provider, resolver, package source, network client, or secret resolver. A changed target, manager binding, source-registry definition, resolution reference, or artifact list invalidates the approved state.

## Controlled APT resolution

The production APT resolver is limited to Debian and Ubuntu targets with an
exact binary name, version, architecture, suite, component set, and
CommonKit-owned source-registry entry. It probes the distro, codename,
architecture, `apt` and `dpkg` versions and no-follow `apt-get`, `apt-cache`,
and `dpkg` executable digests, effective APT
configuration, and the configured `Signed-By` key before repository access. A
mismatch fails before `apt-get update`.

APT receives a CommonKit-owned minimal configuration root, private lists,
archive and binary caches, and empty closure-selection status, one generated source, a scrubbed
environment, and fixed arguments. When running as root, CommonKit requires the
`_apt` identity and gives it group-read/traverse access only to the generated
configuration, source, and key plus ownership of the two APT partial-download
directories. APT is pinned to that sandbox identity; an unsandboxed-root fallback
diagnostic fails resolution. Host configuration fragments, hooks,
automatic proxy discovery and helper or solver overrides are absent and the
effective configuration is checked before repository access. Insecure, weak,
or downgraded-to-insecure repositories, unauthenticated packages, proxies, and
automatic redirects are disabled. Resolution invokes only metadata update,
dependency inspection, simulation, and `--print-uris --download-only`; it
never invokes an install. Simulation and archive enumeration share the same
empty private solver state and must return the same exact closure. Separately,
CommonKit opens the root-owned dpkg status without following the leaf, copies it
into the private workspace, and requires a zero-exit, complete `dpkg-query`
inventory to match it. Any nonterminal or error-state dpkg record rejects the
resolution before repository access. A second non-networked simulation pins
every archived package and version against that live-state snapshot. Every
install or configure action must match an exact name, architecture, and version
in that closure; duplicate or malformed actions fail closed. It must produce no
removal, downgrade, replacement, or unresolved outcome. The canonical installed
inventory, status digest, configure actions, and
normalized safety result are bound into the before observation. The original
status digest is rechecked after resolution.

The resolver opens the configured signing key once without following the
leaf, requires a root-owned regular file with no group/world write bits,
copies those bytes into the private workspace, and points `Signed-By` only at
that immutable copy. The source registry binds the suite, component set,
signer scope, and key digest, so widening or re-keying invalidates existing
resolution authority. The resolver requires one authenticated `InRelease`,
binds its digest, rejects removal/downgrade/replacement relationships or
unresolved dependency alternatives from real APT output, and accepts only
exact SHA-256 `.deb` records whose complete repository-relative `Filename`
matches the archive locator. CommonKit preflights the complete locator set
before the first request, then downloads every archive through the per-hop
scoped fetch seam and verifies its digest and size before persistence.

## Controlled nvm/Node resolution

The production Node resolver is limited to an exact `NodeRuntime` selector,
the `nodejs-nvm` source, nvm 0.40.6 or newer, and Node's version-specific
`nodejs.org/dist/vX.Y.Z` release directory. It supports only mapped macOS and
glibc Linux binary targets. Windows, musl, aliases, ranges, moving release
paths, custom mirrors, and source-build fallback fail closed.

Before network access, CommonKit opens `nvm.sh`, the configured shell, and the
Node release keyring without following the leaf and binds their digests with
the target tuple and nvm directory. It rejects `default-packages`, a user
`.npmrc` prefix, prefix or mirror environment overrides, package migration,
and latest-npm behavior. nvm remains a sourced shell function; CommonKit never
treats it as a standalone executable.

The resolver preflights the checksum, armored-signature, and exact archive
locators together. It fetches each response through the scoped per-hop seam,
extracts the signed checksum payload with a fixed typed `gpgv` invocation, and
requires the signer (including a signing subkey's primary fingerprint) to be
in CommonKit's pinned Node release-key set. The exact platform archive must
appear once in that signed manifest and its downloaded bytes must match the
signed SHA-256 digest.

Node resolutions use the package-resolution v2 schema. The durable recipe
binds the exact cache materialization path, nvm and shell digests, offline mode,
`NVM_NO_SOURCE_FALLBACK=1`, and nvm's per-version lock while disabling npm
upgrade and package migration. The signed checksums, signature, and archive are
all persisted. Planning and restart recovery reopen only those artifacts; no
resolver, verifier, provider, shell, or network capability is available.

## Deferred work

Package consent and target mutation remain separate work. Neither the APT nor
Node resolver installs packages, creates operations, or writes receipts.
Winget is not part of this phase.
