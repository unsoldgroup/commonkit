export type PushOptInState = "off" | "available" | "on";

export function decodeVapidPublicKey(value: string) {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
}

export function pushOptInState(input: {
  supported: boolean;
  hubEnabled: boolean;
  permission: NotificationPermission;
  subscribed: boolean;
}): PushOptInState {
  if (!input.supported || !input.hubEnabled || input.permission === "denied") return "off";
  return input.subscribed ? "on" : "available";
}
