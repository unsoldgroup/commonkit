import { startHub } from "./server.js";
import { createPushSender } from "./push.js";
import { createAccessVerifier } from "./access.js";

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

function pushOptions() {
  const publicKey = process.env.SESSION_BOARD_VAPID_PUBLIC;
  const privateKey = process.env.SESSION_BOARD_VAPID_PRIVATE;
  const subject = process.env.SESSION_BOARD_VAPID_SUBJECT;
  if (!publicKey || !privateKey || !subject) return undefined;
  return { publicKey, sender: createPushSender(subject, publicKey, privateKey) };
}

/**
 * Both variables or neither. Configuring one without the other would silently leave browser
 * sessions on the token prompt, which is exactly the failure this is meant to remove.
 */
function accessVerifier() {
  const teamDomain = process.env.SESSION_BOARD_ACCESS_TEAM_DOMAIN;
  const aud = process.env.SESSION_BOARD_ACCESS_AUD;
  if (!teamDomain && !aud) return undefined;
  if (!teamDomain || !aud) {
    throw new Error(
      "SESSION_BOARD_ACCESS_TEAM_DOMAIN and SESSION_BOARD_ACCESS_AUD must be set together",
    );
  }
  return createAccessVerifier({ teamDomain, aud });
}

const hub = startHub({
  reporterTokens: reporterTokens(),
  actionToken: required("SESSION_BOARD_ACTION_TOKEN"),
  hostname: process.env.SESSION_BOARD_HOST ?? "127.0.0.1",
  port: Number(process.env.SESSION_BOARD_PORT ?? 8787),
  push: pushOptions(),
  access: accessVerifier(),
});

console.log(`Session Board hub listening on ${hub.url}`);
