# npm distribution

The public `commonkit` package is a small Node.js launcher for the production
Rust runtime. It does not contain the legacy JavaScript reconciliation
prototype and does not download executables during installation.

The launcher exposes:

```text
commonkit
commonkitd
commonkit-target-helper
```

It selects one optional native dependency:

```text
commonkit-darwin-arm64
commonkit-linux-x64
```

Every native package is restricted with npm `os` and `cpu` fields. It contains
the three release executables and a `checksums.json` file. The launcher verifies
the selected executable against its SHA-256 digest before every launch.

## Build and pack

Build the three Rust executables on the native platform. Stage a package into
an empty private directory:

```sh
node scripts/stage-npm-native-package.mjs \
  commonkit-darwin-arm64 \
  target/release \
  /tmp/commonkit-darwin-arm64
```

Pack the staged native package and launcher:

```sh
npm pack /tmp/commonkit-darwin-arm64 --pack-destination /tmp/commonkit-npm
pnpm --dir packages/commonkit-npm pack --pack-destination /tmp/commonkit-npm
```

Linux uses `commonkit-linux-x64` and must be built on the qualified Linux
runner. Do not relabel cross-compiled or unqualified executables as native
release payloads.

## Verify before publication

Install the native tarball first into an isolated prefix, then install the
launcher with optional dependency resolution disabled:

```sh
npm install --global --prefix /tmp/commonkit-prefix \
  /tmp/commonkit-npm/commonkit-darwin-arm64-0.1.0.tgz
npm install --global --prefix /tmp/commonkit-prefix --omit=optional \
  /tmp/commonkit-npm/commonkit-0.1.0.tgz
/tmp/commonkit-prefix/bin/commonkit --version
/tmp/commonkit-prefix/bin/commonkitd --help
```

The package contract tests also inspect the packed allowlist, platform
restrictions, launcher behavior, forwarded exit status, unsupported-platform
diagnostic, staged executable set, and tamper rejection:

```sh
node --test test/npm-distribution.test.mjs
```

## Publish

Authenticate the trusted release workstation with npm. Publish native packages
before the launcher so a fresh installation can resolve its optional
dependency:

```sh
npm publish commonkit-darwin-arm64-0.1.0.tgz --access public
npm publish commonkit-linux-x64-0.1.0.tgz --access public
npm publish commonkit-0.1.0.tgz --access public
```

After publication, install `commonkit@0.1.0` into a fresh prefix from the
registry and repeat the version and daemon checks. npm publication is manual;
CommonKit does not use GitHub Actions.
