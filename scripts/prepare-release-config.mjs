import { readFile, writeFile } from "node:fs/promises";

const required = ["TAURI_UPDATER_PUBLIC_KEY", "COMMONKIT_UPDATE_ENDPOINT"];
const missing = required.filter((name) => !process.env[name]?.trim());

if (missing.length) {
  console.error(`release configuration missing: ${missing.join(", ")}`);
  process.exitCode = 1;
} else {
  const path = new URL("../apps/desktop/src-tauri/tauri.conf.json", import.meta.url);
  const config = JSON.parse(await readFile(path, "utf8"));
  config.plugins ??= {};
  config.plugins.updater = {
    pubkey: process.env.TAURI_UPDATER_PUBLIC_KEY.trim(),
    endpoints: [process.env.COMMONKIT_UPDATE_ENDPOINT.trim()],
  };
  await writeFile(path, `${JSON.stringify(config, null, 2)}\n`);
}
