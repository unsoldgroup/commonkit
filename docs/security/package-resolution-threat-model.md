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

APT receives a CommonKit-owned minimal configuration root, private lists,
archive cache and empty closure-selection status, one generated source, a scrubbed
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
inventory to match it. A second non-networked simulation pins every archived
package and version against that live-state snapshot. It must select no package
outside the archive set and produce no removal, downgrade, replacement, or
unresolved outcome. The canonical installed inventory, status digest, and
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
exact SHA-256 `.deb` records. CommonKit preflights the complete locator set
before the first request, then downloads every archive through the per-hop
scoped fetch seam and verifies its digest and size before persistence.

## Deferred work

Concrete Node resolution backends, consent, and package mutation remain
separate work. The APT resolver does not install packages, create operations,
or write receipts. Winget is not part of this phase.
