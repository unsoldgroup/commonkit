import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = dirname(fileURLToPath(import.meta.url));
const canaryRoot = join(packageRoot, "canary");
const require = createRequire(import.meta.url);
const configPackage = JSON.parse(
  readFileSync(join(packageRoot, "package.json"), "utf8"),
);
const reviewedOxlintVersion = configPackage.peerDependencies.oxlint;
const nativeConfigPath = require.resolve("@commonkit/oxlint-config/native");
assert.equal(nativeConfigPath, join(packageRoot, "native.json"));
const oxlintPackagePath = require.resolve("oxlint/package.json");
const oxlintPackage = JSON.parse(readFileSync(oxlintPackagePath, "utf8"));
const oxlintBin = join(dirname(oxlintPackagePath), oxlintPackage.bin.oxlint);

function run(args) {
  return spawnSync(process.execPath, [oxlintBin, ...args], {
    cwd: packageRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

const version = run(["--version"]);
assert.equal(version.status, 0, version.stderr);
const versionMatch = version.stdout.match(/(\d+\.\d+\.\d+)/);
assert.ok(versionMatch, `unexpected Oxlint version output: ${version.stdout}`);
assert.equal(versionMatch[1], reviewedOxlintVersion);

const config = join(canaryRoot, ".oxlintrc.json");
const valid = run(["--config", config, join(canaryRoot, "valid.js")]);
assert.equal(valid.status, 0, valid.stderr);

const invalid = run([
  "--config",
  config,
  "--format",
  "json",
  join(canaryRoot, "invalid.js"),
]);
assert.equal(invalid.status, 1, invalid.stderr);
const invalidReport = JSON.parse(invalid.stdout);
assert.equal(invalidReport.diagnostics.length, 1);
assert.equal(invalidReport.diagnostics[0].code, "eslint(no-debugger)");

const ignored = run([
  "--config",
  config,
  join(canaryRoot, "valid.js"),
  join(canaryRoot, "ignored.js"),
]);
assert.equal(ignored.status, 0, ignored.stderr);

process.stdout.write(
  `${JSON.stringify({
    schemaVersion: 1,
    platform: process.platform,
    architecture: process.arch,
    nodeVersion: process.version,
    oxlintVersion: versionMatch[1],
    reviewedOxlintVersion,
    configExportResolved: true,
    configLoaded: true,
    validExitCode: valid.status,
    invalidExitCode: invalid.status,
    ignoredExitCode: ignored.status,
    jsonDiagnosticRule: invalidReport.diagnostics[0].code.replace(
      /^eslint\(|\)$/g,
      "",
    ),
  })}\n`,
);
