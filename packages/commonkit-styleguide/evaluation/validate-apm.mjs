import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdtempSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const stagingRoot = mkdtempSync(join(tmpdir(), "commonkit-styleguide-apm-"));
const stagedPackage = join(stagingRoot, basename(packageRoot));
const apm = process.env.APM_BIN || "apm";

function run(args) {
  return execFileSync(apm, args, {
    cwd: stagedPackage,
    encoding: "utf8",
    env: { ...process.env, APM_PROGRESS: "never" },
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

try {
  cpSync(packageRoot, stagedPackage, {
    recursive: true,
    filter: (source) =>
      !source.includes(`${join(packageRoot, "node_modules")}`),
  });

  const version = run(["--version"]).trim();
  assert.match(version, /\b0\.25\.0\b/, "APM must be pinned to 0.25.0");

  run(["install", "--target", "claude,codex"]);
  run(["install", "--frozen", "--target", "claude,codex"]);
  const audit = JSON.parse(run(["audit", "--ci", "--format", "json"]));
  assert.equal(audit.passed, true, "APM audit must pass");
  assert.equal(audit.summary.failed, 0, "APM audit must have no failed checks");

  const source = digest(
    join(stagedPackage, ".apm/skills/technical-writing/SKILL.md"),
  );
  const claude = digest(
    join(stagedPackage, ".claude/skills/technical-writing/SKILL.md"),
  );
  const codex = digest(
    join(stagedPackage, ".agents/skills/technical-writing/SKILL.md"),
  );
  assert.equal(claude, source, "Claude must receive exact source bytes");
  assert.equal(codex, source, "Codex must receive exact source bytes");

  process.stdout.write(
    `${JSON.stringify(
      {
        schemaVersion: 1,
        platform: process.platform,
        architecture: process.arch,
        nodeVersion: process.version,
        apmVersion: version,
        targets: ["claude", "codex"],
        skillDigest: `sha256:${source}`,
        auditChecks: audit.summary.total,
        auditPassed: audit.summary.passed,
        frozenReplay: "passed",
      },
      null,
      2,
    )}\n`,
  );
} finally {
  rmSync(stagingRoot, { recursive: true, force: true });
}
