import webPush from "web-push";

import type { PushSender } from "./server.js";

export function createPushSender(subject: string, publicKey: string, privateKey: string): PushSender {
  webPush.setVapidDetails(subject, publicKey, privateKey);
  return {
    async send(subscription, payload) {
      await webPush.sendNotification(subscription, payload);
    },
  };
}
