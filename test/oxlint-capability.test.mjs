import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";

const rootPackage = JSON.parse(readFileSync("package.json", "utf8"));
const projectConfig = JSON.parse(readFileSync(".oxlintrc.json", "utf8"));
const nativeBaseline = JSON.parse(
  readFileSync("packages/commonkit-oxlint-config/native.json", "utf8"),
);

test("pins Oxlint and exposes repository-owned lint commands", () => {
  assert.equal(rootPackage.devDependencies.oxlint, "1.79.0");
  assert.equal(
    rootPackage.scripts["lint:oxlint"],
    "oxlint --config .oxlintrc.json apps packages scripts src test",
  );
  assert.equal(
    rootPackage.scripts["test:oxlint-canary"],
    "pnpm --filter @commonkit/oxlint-config test",
  );
});

test("keeps project-only Oxlint fields outside the shared baseline", () => {
  assert.deepEqual(projectConfig.extends, [
    "./packages/commonkit-oxlint-config/native.json",
  ]);
  assert.deepEqual(Object.keys(nativeBaseline).sort(), [
    "overrides",
    "plugins",
    "rules",
  ]);

  for (const field of [
    "categories",
    "env",
    "ignorePatterns",
    "settings",
    "options",
  ]) {
    assert.ok(
      Object.hasOwn(projectConfig, field),
      `project config must own ${field}`,
    );
    assert.equal(
      Object.hasOwn(nativeBaseline, field),
      false,
      `shared baseline must not own ${field}`,
    );
  }
});

test("the native compatibility canary proves version, config, and exit behavior", () => {
  const output = execFileSync(
    process.execPath,
    ["packages/commonkit-oxlint-config/canary.mjs"],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
  const evidence = JSON.parse(output);

  assert.equal(evidence.oxlintVersion, "1.79.0");
  assert.equal(evidence.reviewedOxlintVersion, "1.79.0");
  assert.equal(evidence.configExportResolved, true);
  assert.equal(evidence.configLoaded, true);
  assert.equal(evidence.validExitCode, 0);
  assert.equal(evidence.invalidExitCode, 1);
  assert.equal(evidence.ignoredExitCode, 0);
  assert.equal(evidence.jsonDiagnosticRule, "no-debugger");
});
