#!/usr/bin/env node
// Generate the native provider file mappings for a populated kit.
//
// The native provider maps one kit file to one managed path, so a kit carrying
// N skill files needs N entries per agent layout. Writing those by hand does not
// scale past the first skill, and they must be regenerated whenever the kit
// changes because the pipeline pins an exact revision.
//
//   node scripts/kit-native-mappings.mjs [kit-dir] > mappings.json
//
// Emits {revision, provider, version, files:[{path, source}]} ready to splice
// into headless.json at sync.providerPipeline.providers[].

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const kit = process.argv[2] ?? join(homedir(), ".commonkit-kit");
if (!existsSync(join(kit, ".git"))) {
  throw new Error(`not a kit checkout: ${kit}`);
}

// The pipeline reads committed content, never the working tree, so a dirty kit
// would produce mappings for files the daemon cannot see.
const dirty = execFileSync("git", ["-C", kit, "status", "--porcelain"], { encoding: "utf8" }).trim();
if (dirty) {
  throw new Error(`kit has uncommitted changes; commit first:\n${dirty}`);
}
const revision = execFileSync("git", ["-C", kit, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();

/** Tracked files only. The pipeline reads the committed revision, so anything
 *  git ignores (.DS_Store and friends) must never become a managed path. */
function tracked(prefix) {
  const listed = execFileSync("git", ["-C", kit, "ls-files", "-z", "--", prefix], { encoding: "utf8" });
  return listed.split("\0").filter(Boolean);
}

/** Both layouts, because Claude reads .claude/ and Codex reads .agents/. */
const LAYOUTS = [
  { from: "skills", to: ["agent-context/.claude/skills", "agent-context/.agents/skills"] },
  { from: "agents", to: ["agent-context/.claude/agents", "agent-context/.agents/claude-agents"] },
];

const files = LAYOUTS.flatMap(({ from, to }) =>
  tracked(from).flatMap((source) => {
    const tail = source.slice(from.length + 1);
    return to.map((root) => ({ path: `${root}/${tail}`, source }));
  }),
);

// The file adapter rejects these outright; catching it here beats a failed plan.
const unsafe = files.filter(({ path }) =>
  /[\\:]/.test(path) || path.split("/").some((segment) => !segment || segment === ".git"),
);
if (unsafe.length) {
  throw new Error(`${unsafe.length} unsafe managed path(s), first: ${unsafe[0].path}`);
}

process.stdout.write(`${JSON.stringify({ revision, provider: "native", version: "0.1.0", files }, null, 2)}\n`);
process.stderr.write(`${files.length} mappings at ${revision.slice(0, 7)}\n`);
