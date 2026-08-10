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
architecture, `apt` and `dpkg` versions and executable digests, effective APT
configuration, and the configured `Signed-By` key before repository access. A
mismatch fails before `apt-get update`.

APT receives private lists and archive-cache directories, a single generated
source file, a scrubbed environment, and fixed arguments. Insecure, weak, or
downgraded-to-insecure repositories, unauthenticated packages, proxies, and
automatic redirects are disabled. Resolution invokes only metadata update,
simulation, and `--print-uris --download-only`; it never invokes an install.
The resolver requires one authenticated `InRelease`, binds its digest and the
no-follow key-file digest, rejects held/removal/downgrade/replacement or
unresolved-alternative outcomes, and accepts only exact SHA-256 `.deb`
records. CommonKit preflights the complete locator set before the first
request, then downloads every archive through the per-hop scoped fetch seam
and verifies its digest and size before persistence.

## Deferred work

Concrete Node resolution backends, consent, and package mutation remain
separate work. The APT resolver does not install packages, create operations,
or write receipts. Winget is not part of this phase.
