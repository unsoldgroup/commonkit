import { startHub } from "./server.js";

function required(name: string) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function reporterTokens() {
  const value = JSON.parse(required("SESSION_BOARD_REPORTER_TOKENS"));
  if (!value || Array.isArray(value) || typeof value !== "object") {
    throw new Error("SESSION_BOARD_REPORTER_TOKENS must be a JSON object of Target ids to tokens");
  }
  for (const [machineId, token] of Object.entries(value)) {
    if (!machineId || typeof token !== "string" || !token) {
      throw new Error("SESSION_BOARD_REPORTER_TOKENS must contain non-empty Target ids and tokens");
    }
  }
  return value as Record<string, string>;
}

const hub = startHub({
  reporterTokens: reporterTokens(),
  actionToken: required("SESSION_BOARD_ACTION_TOKEN"),
  hostname: process.env.SESSION_BOARD_HOST ?? "127.0.0.1",
  port: Number(process.env.SESSION_BOARD_PORT ?? 8787),
});

console.log(`Session Board hub listening on ${hub.url}`);
