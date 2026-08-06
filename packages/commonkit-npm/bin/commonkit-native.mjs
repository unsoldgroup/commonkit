#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, join } from "node:path";

const commands = new Set([
  "commonkit",
  "commonkitd",
  "commonkit-target-helper",
]);
const invoked = basename(process.argv[1]).replace(/\.cmd$/i, "");
const command = commands.has(invoked) ? invoked : "commonkit";
const packageName = `commonkit-${process.platform}-${process.arch}`;
const require = createRequire(import.meta.url);

let manifest;
try {
  manifest = require.resolve(`${packageName}/package.json`);
} catch {
  console.error(
    `CommonKit does not have an installed native package for ${process.platform}/${process.arch}. ` +
      `Expected optional dependency ${packageName}.`,
  );
  process.exit(1);
}

const executable = join(
  dirname(manifest),
  "bin",
  `${command}${process.platform === "win32" ? ".exe" : ""}`,
);
const checksums = JSON.parse(
  readFileSync(join(dirname(manifest), "checksums.json"), "utf8"),
);
const expected = checksums[command];
const actual = `sha256:${createHash("sha256")
  .update(readFileSync(executable))
  .digest("hex")}`;
if (!expected || actual !== expected) {
  console.error(`${command} failed its SHA-256 integrity check.`);
  process.exit(1);
}
const child = spawn(executable, process.argv.slice(2), { stdio: "inherit" });

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => child.kill(signal));
}
child.on("error", (error) => {
  console.error(`Unable to start ${command}: ${error.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
