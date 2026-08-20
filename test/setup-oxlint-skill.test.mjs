import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const inspector =
  "packages/commonkit-oxlint-config/skills/setup-oxlint/scripts/inspect-project.mjs";

function inspect(manifest, extraFiles = []) {
  const root = mkdtempSync(join(tmpdir(), "setup-oxlint-skill-"));
  try {
    writeFileSync(join(root, "package.json"), JSON.stringify(manifest));
    writeFileSync(join(root, "pnpm-lock.yaml"), "lockfileVersion: '9.0'\n");
    for (const file of extraFiles) {
      writeFileSync(join(root, file), "export default {};\n");
    }
    return JSON.parse(
      execFileSync(process.execPath, [inspector, "--root", root], {
        encoding: "utf8",
      }),
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("classifies legacy ESLint repositories for a pinned dual run", () => {
  const result = inspect(
    {
      devDependencies: {
        eslint: "8.57.1",
        "eslint-plugin-local": "workspace:*",
        prettier: "3.6.2",
      },
      scripts: { lint: "eslint ." },
    },
    [".eslintrc.js"],
  );

  assert.equal(result.packageManager, "pnpm");
  assert.equal(result.migrationTrack, "eslint-dual-run");
  assert.equal(result.reviewedOxlintVersion, "1.79.0");
  assert.deepEqual(result.eslintConfigs, [".eslintrc.js"]);
  assert.deepEqual(result.customLintPackages, ["eslint-plugin-local"]);
  assert.equal(result.preserveFormatter, true);
});

test("keeps Biome repositories on their current tool unless owners opt in", () => {
  const result = inspect({
    devDependencies: { "@biomejs/biome": "2.2.0" },
    scripts: { lint: "biome check ." },
  });

  assert.equal(result.migrationTrack, "preserve-biome");
});

test("inventories scoped ESLint plugins without misclassifying ESLint internals", () => {
  const result = inspect(
    {
      devDependencies: {
        eslint: "9.34.0",
        "@eslint/config-array": "0.21.0",
        "@next/eslint-plugin-next": "15.5.0",
        "@typescript-eslint/eslint-plugin": "8.41.0",
        "@company/eslint-plugin-local": "1.0.0",
      },
      eslintConfig: { extends: ["next"] },
    },
    [".eslintrc.yml"],
  );

  assert.deepEqual(result.eslintConfigs, [
    ".eslintrc.yml",
    "package.json#eslintConfig",
  ]);
  assert.deepEqual(result.customLintPackages, [
    "@company/eslint-plugin-local",
    "@next/eslint-plugin-next",
    "@typescript-eslint/eslint-plugin",
  ]);
});

test("ships explicit anti-slop provenance fields for reviewed vendoring", () => {
  const provenance = JSON.parse(
    readFileSync(
      "packages/commonkit-oxlint-config/skills/setup-oxlint/assets/anti-slop.provenance.json",
      "utf8",
    ),
  );

  assert.equal(provenance.upstream.commit, "6d538555cb151d4121ed51a27db81890eacf8ae9");
  assert.equal(provenance.commonkitNativeBaseline.oxlint, "1.79.0");
  assert.equal(provenance.upstream.compatibility.oxlint, "1.78.0");
  assert.equal(provenance.upstream.compatibility["@oxlint/plugins"], "1.78.0");
  assert.deepEqual(provenance.upstream.licenseNotice, {
    sourcePath: "LICENSE",
    destination: null,
    sha256: null,
  });
  assert.equal(provenance.review.status, "pending");
  assert.deepEqual(provenance.review.requiredClassifications, [
    "defect",
    "useful-policy",
    "acceptable-exception",
    "false-positive",
  ]);
  assert.deepEqual(provenance.files, {});
});
