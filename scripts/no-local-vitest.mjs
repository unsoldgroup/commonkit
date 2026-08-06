#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { dirname, join, parse, resolve } from "node:path";

let raw = "";
for await (const chunk of process.stdin) raw += chunk;

let payload;
try {
  payload = JSON.parse(raw || "{}");
} catch {
  process.exit(0);
}

const input = payload.tool_input ?? payload.toolInput ?? {};
const command = input.command ?? input.cmd;
if (typeof command !== "string" || /(^|\s)rtest(?:\s|$)/.test(command)) {
  process.exit(0);
}

const directVitest =
  /(^|[;&|()]|\s)(?:(?:pnpm|pnpx|npx)\s+(?:exec\s+)?vitest|vitest)(?=\s|$)/;

function findPackageJson(start) {
  let current = resolve(start);
  const root = parse(current).root;
  while (true) {
    try {
      return JSON.parse(readFileSync(join(current, "package.json"), "utf8"));
    } catch (error) {
      if (error?.code !== "ENOENT" && !(error instanceof SyntaxError)) return undefined;
    }
    if (current === root) return undefined;
    current = dirname(current);
  }
}

function invokesVitestScript() {
  const match = command.match(/(?:^|[;&|()]|\s)pnpm\s+(?:run\s+)?([A-Za-z0-9:_-]+)(?=\s|$)/);
  if (!match) return false;
  const cwd = input.cwd ?? payload.cwd ?? process.cwd();
  const packageJson = findPackageJson(cwd);
  const scripts = packageJson?.scripts ?? {};
  const visited = new Set();

  function resolvesToVitest(scriptName) {
    if (visited.has(scriptName)) return false;
    visited.add(scriptName);
    const script = scripts[scriptName] ?? "";
    if (directVitest.test(script)) return true;
    const nested = script.match(/(?:^|[;&|()]|\s)pnpm\s+(?:run\s+)?([A-Za-z0-9:_-]+)(?=\s|$)/);
    return nested ? resolvesToVitest(nested[1]) : false;
  }

  return resolvesToVitest(match[1]);
}

if (directVitest.test(command) || invokesVitestScript()) {
  process.stderr.write(`Local Vitest is disabled to protect Mac memory. Use: rtest ${command}\n`);
  process.exit(2);
}
