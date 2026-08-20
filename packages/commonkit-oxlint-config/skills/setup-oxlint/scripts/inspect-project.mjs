import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const rootIndex = process.argv.indexOf("--root");
if (rootIndex === -1 || !process.argv[rootIndex + 1]) {
  throw new Error("usage: node inspect-project.mjs --root <repository>");
}

const root = resolve(process.argv[rootIndex + 1]);
const manifestPath = resolve(root, "package.json");
if (!existsSync(manifestPath)) {
  throw new Error(`package.json not found under ${root}`);
}

const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const dependencies = {
  ...manifest.dependencies,
  ...manifest.devDependencies,
};
const scripts = manifest.scripts ?? {};

const lockfiles = [
  ["pnpm", "pnpm-lock.yaml"],
  ["npm", "package-lock.json"],
  ["yarn", "yarn.lock"],
  ["bun", "bun.lock"],
];
const packageManager =
  lockfiles.find(([, file]) => existsSync(resolve(root, file)))?.[0] ?? null;

const eslintConfigs = [
  "eslint.config.js",
  "eslint.config.mjs",
  "eslint.config.cjs",
  "eslint.config.ts",
  ".eslintrc",
  ".eslintrc.js",
  ".eslintrc.cjs",
  ".eslintrc.json",
  ".eslintrc.yaml",
  ".eslintrc.yml",
].filter((file) => existsSync(resolve(root, file)));
if (Object.hasOwn(manifest, "eslintConfig")) {
  eslintConfigs.push("package.json#eslintConfig");
}

const hasBiome = Object.hasOwn(dependencies, "@biomejs/biome");
const hasEslint = Object.hasOwn(dependencies, "eslint") || eslintConfigs.length > 0;
const hasOxlint = Object.hasOwn(dependencies, "oxlint");

let migrationTrack = "new-oxlint";
if (hasBiome) migrationTrack = "preserve-biome";
else if (hasEslint) migrationTrack = "eslint-dual-run";
else if (hasOxlint) migrationTrack = "existing-oxlint";

const customLintPackages = Object.keys(dependencies)
  .filter(
    (name) =>
      (name.startsWith("eslint-plugin-") ||
        /^@[^/]+\/eslint-plugin(?:-.+)?$/.test(name)) &&
      name !== "eslint-plugin-oxlint",
  )
  .sort();

process.stdout.write(
  `${JSON.stringify({
    schemaVersion: 1,
    root,
    packageManager,
    migrationTrack,
    reviewedOxlintVersion: "1.79.0",
    existingOxlintVersion: dependencies.oxlint ?? null,
    eslintConfigs,
    customLintPackages,
    preserveFormatter:
      hasBiome ||
      Object.hasOwn(dependencies, "prettier") ||
      Object.keys(scripts).some((name) => name.startsWith("format")),
    scripts,
  })}\n`,
);
