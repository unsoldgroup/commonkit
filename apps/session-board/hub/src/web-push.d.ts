declare module "web-push" {
  interface WebPushSubscription {
    endpoint: string;
    expirationTime?: number | null;
    keys: { p256dh: string; auth: string };
  }

  interface WebPushError extends Error {
    statusCode: number;
  }

  const webPush: {
    setVapidDetails(subject: string, publicKey: string, privateKey: string): void;
    sendNotification(subscription: WebPushSubscription, payload?: string): Promise<unknown>;
  };

  export default webPush;
}
