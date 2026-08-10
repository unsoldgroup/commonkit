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

## Deferred work

This phase defines the controlled-resolution contract and trust boundary. Concrete Apt and Node resolution backends, consent, and package mutation remain separate work. Winget is not part of this phase.
