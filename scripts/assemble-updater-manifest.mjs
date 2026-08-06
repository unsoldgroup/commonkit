import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const [root, rawTag, notesPath] = process.argv.slice(2);
if (!root || !rawTag || !notesPath) throw new Error("usage: assemble-updater-manifest.mjs <root> <tag> <release-notes-file>");
const version = rawTag.replace(/^refs\/tags\//, "").replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) throw new Error("invalid release version");

const names = await readdir(root);
const notes = (await readFile(notesPath, "utf8")).trim();
if (!notes) throw new Error("release notes must not be empty");
const macPayload = names.find((name) => name.endsWith(".app.tar.gz"));
const definitions = [
  ["darwin-aarch64", macPayload],
  ["darwin-x86_64", macPayload],
  ["linux-x86_64", names.find((name) => name.endsWith(".AppImage"))],
  ["windows-x86_64", names.find((name) => name.endsWith(".nsis.zip"))],
];
const base = process.env.COMMONKIT_RELEASE_DOWNLOAD_BASE;
if (!base?.startsWith("https://")) throw new Error("COMMONKIT_RELEASE_DOWNLOAD_BASE must be HTTPS");
const platforms = {};
for (const [platform, name] of definitions) {
  if (!name) throw new Error(`updater payload missing for ${platform}`);
  const signatureName = `${name}.sig`;
  if (!names.includes(signatureName)) throw new Error(`updater signature missing: ${signatureName}`);
  const signature = (await readFile(join(root, signatureName), "utf8")).trim();
  if (!signature) throw new Error(`updater signature is empty: ${signatureName}`);
  platforms[platform] = {
    url: `${base.replace(/\/$/, "")}/${encodeURIComponent(name)}`,
    signature,
  };
}
await writeFile(join(root, "latest.json"), `${JSON.stringify({ version, notes, platforms }, null, 2)}\n`);
