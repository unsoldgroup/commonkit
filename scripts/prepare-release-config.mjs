import { readFile, writeFile } from "node:fs/promises";

const required = ["TAURI_UPDATER_PUBLIC_KEY", "COMMONKIT_UPDATE_ENDPOINT"];
if (process.env.RUNNER_OS === "Windows") {
  required.push("WINDOWS_CERTIFICATE_THUMBPRINT", "WINDOWS_TIMESTAMP_URL");
}
const missing = required.filter((name) => !process.env[name]?.trim());

if (missing.length) {
  console.error(`release configuration missing: ${missing.join(", ")}`);
  process.exitCode = 1;
} else {
  const path = new URL("../apps/desktop/src-tauri/tauri.conf.json", import.meta.url);
  const config = JSON.parse(await readFile(path, "utf8"));
  const rawTag = process.argv[2];
  if (!rawTag?.startsWith("v") || rawTag.slice(1) !== config.version) {
    throw new Error(`release tag ${rawTag ?? "<missing>"} must match desktop version v${config.version}`);
  }
  const updateEndpoint = process.env.COMMONKIT_UPDATE_ENDPOINT.trim();
  if (!updateEndpoint.startsWith("https://")) {
    throw new Error("COMMONKIT_UPDATE_ENDPOINT must use HTTPS");
  }
  config.plugins ??= {};
  config.plugins.updater = {
    pubkey: process.env.TAURI_UPDATER_PUBLIC_KEY.trim(),
    endpoints: [updateEndpoint],
  };
  if (process.env.RUNNER_OS === "Windows") {
    const timestampUrl = process.env.WINDOWS_TIMESTAMP_URL.trim();
    if (!timestampUrl.startsWith("https://")) {
      throw new Error("WINDOWS_TIMESTAMP_URL must use HTTPS");
    }
    config.bundle.windows = {
      ...(config.bundle.windows ?? {}),
      certificateThumbprint: process.env.WINDOWS_CERTIFICATE_THUMBPRINT.trim(),
      digestAlgorithm: "sha256",
      timestampUrl,
    };
  }
  await writeFile(path, `${JSON.stringify(config, null, 2)}\n`);
}
