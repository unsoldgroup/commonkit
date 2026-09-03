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

/**
 * Strip regions that carry prose rather than commands.
 *
 * The hook matches against the whole command string, so any command that merely
 * DESCRIBES a Vitest run was blocked: `gh pr create` with a body documenting the
 * test command, `git commit -m "... vitest ..."`, `echo`. Measured 2026-09-03: a
 * PR body reading "rtest pnpm exec vitest related" blocked `gh pr create`, and
 * the leading backtick meant the rtest allowance on the whole command did not
 * apply either.
 *
 * Heredoc bodies and the values of text-carrying flags are never command
 * position, so removing them cannot hide a real invocation. Ordinary quoted
 * strings are deliberately NOT stripped: `ssh host "pnpm exec vitest"` is a real
 * run that should still be caught.
 */
function stripProse(text) {
  return (
    text
      // <<EOF ... EOF and <<'EOF' ... EOF (also <<-)
      .replace(/<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1[\s\S]*?^\s*\2\s*$/gm, " ")
      // an unterminated heredoc still must not leak its body into the match
      .replace(/<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1[\s\S]*$/, " ")
      // -m/--message/--body/--title/--description/--notes "..." or '...'
      .replace(
        /(?:^|\s)(?:-m|--message|--body|--title|--description|--notes)(?:=|\s+)(["'])[\s\S]*?\1/g,
        " ",
      )
  );
}

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

function invokesVitestScript(text) {
  const match = text.match(/(?:^|[;&|()]|\s)pnpm\s+(?:run\s+)?([A-Za-z0-9:_-]+)(?=\s|$)/);
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

const scannable = stripProse(command);

if (directVitest.test(scannable) || invokesVitestScript(scannable)) {
  process.stderr.write(`Local Vitest is disabled to protect Mac memory. Use: rtest ${command}\n`);
  process.exit(2);
}
