import { describe, expect, test } from "bun:test";
import { decodeVapidPublicKey, pushOptInState } from "../src/push.js";

describe("push opt-in", () => {
  test("decodes a base64url VAPID public key", () => {
    expect([...decodeVapidPublicKey("AQID-v8")]).toEqual([1, 2, 3, 250, 255]);
  });

  test("derives an off state when push cannot be used", () => {
    expect(pushOptInState({ supported: false, hubEnabled: true, permission: "default", subscribed: false })).toBe("off");
    expect(pushOptInState({ supported: true, hubEnabled: false, permission: "default", subscribed: false })).toBe("off");
    expect(pushOptInState({ supported: true, hubEnabled: true, permission: "denied", subscribed: false })).toBe("off");
  });

  test("distinguishes an available opt-in from an active subscription", () => {
    expect(pushOptInState({ supported: true, hubEnabled: true, permission: "default", subscribed: false })).toBe("available");
    expect(pushOptInState({ supported: true, hubEnabled: true, permission: "granted", subscribed: true })).toBe("on");
  });
});
