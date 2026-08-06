#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  chmod,
  copyFile,
  lstat,
  mkdir,
  readFile,
  readdir,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const [, , packageName, binaryRoot, outputRoot] = process.argv;
const supported = new Set([
  "commonkit-darwin-arm64",
  "commonkit-linux-x64",
]);
if (!supported.has(packageName) || !binaryRoot || !outputRoot) {
  throw new Error(
    "usage: stage-npm-native-package.mjs <commonkit-darwin-arm64|commonkit-linux-x64> <binary-root> <output-root>",
  );
}

const repositoryRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const existing = await readdir(outputRoot);
if (existing.length > 0) {
  throw new Error("native npm staging destination must be empty");
}
await mkdir(join(outputRoot, "bin"), { recursive: true });

const checksums = {};
for (const name of ["commonkit", "commonkitd", "commonkit-target-helper"]) {
  const source = join(binaryRoot, name);
  const metadata = await lstat(source);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`native npm payload is not a regular file: ${name}`);
  }
  const content = await readFile(source);
  checksums[name] = `sha256:${createHash("sha256").update(content).digest("hex")}`;
  const destination = join(outputRoot, "bin", name);
  await copyFile(source, destination);
  await chmod(destination, 0o755);
}

await copyFile(
  join(repositoryRoot, "packages", packageName, "package.json"),
  join(outputRoot, "package.json"),
);
await copyFile(join(repositoryRoot, "LICENSE"), join(outputRoot, "LICENSE"));
await copyFile(join(repositoryRoot, "README.md"), join(outputRoot, "README.md"));
await writeFile(
  join(outputRoot, "checksums.json"),
  `${JSON.stringify(checksums, null, 2)}\n`,
);
