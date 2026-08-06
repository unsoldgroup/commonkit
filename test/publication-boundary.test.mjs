import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { extname, join, relative } from "node:path";
import test from "node:test";

const repositoryRoot = new URL("..", import.meta.url).pathname;
const textExtensions = new Set([
  "", ".css", ".html", ".js", ".json", ".jsonl", ".md", ".mjs", ".rs",
  ".sh", ".swift", ".toml", ".ts", ".tsx", ".txt", ".xml", ".yaml", ".yml",
]);
const forbiddenPaths = [
  ".engram/",
  "docs/kit-onboarding-handover.md",
];
const privateMarkers = [
  "/Users/" + ["aste", "marie"].join(""),
  "/Users/" + ["a", "l"].join("") + "/",
  ["a", "l-macbook"].join(""),
  ["a", "l-unsoldgroup"].join(""),
  ["srv", "1833518"].join(""),
  ["72.60.", "44.157"].join(""),
  ["100.71.", "109.66"].join(""),
  ["Alexs-", "MacBook-Air.local"].join(""),
];
const ignoredDirectories = new Set([".git", ".cache", "build", "dist", "node_modules", "target"]);

async function repositoryFiles(directory = repositoryRoot) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory() && ignoredDirectories.has(entry.name)) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await repositoryFiles(path));
    else if (entry.isFile()) files.push(relative(repositoryRoot, path));
  }
  return files;
}

test("the tracked product tree contains no private machine or kit configuration", async () => {
  const files = await repositoryFiles();

  const forbiddenTracked = files.filter((file) => forbiddenPaths.some((path) => file === path || file.startsWith(path)));
  assert.deepEqual(forbiddenTracked, [], `private paths are tracked:\n${forbiddenTracked.join("\n")}`);

  const leaks = [];
  for (const file of files) {
    if (!textExtensions.has(extname(file))) continue;
    const source = await readFile(join(repositoryRoot, file), "utf8");
    for (const marker of privateMarkers) {
      if (source.toLowerCase().includes(marker.toLowerCase())) leaks.push(`${file}: ${marker}`);
    }
  }
  assert.deepEqual(leaks, [], `private markers found:\n${leaks.join("\n")}`);
});
