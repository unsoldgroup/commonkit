import { createHash } from "node:crypto";
import { redactSensitiveText } from "./redaction.js";

/** The graph hub owns the shared thematic vocabulary. */
export const THEMES = {
  checkout: "Checkout and payments",
  quotes: "Quotes and customer journeys",
  policies: "Insurance coverage and policy documents",
  pricing: "Pricing and carrier integrations",
  communications: "Customer communications and messaging",
  security: "Security, authentication and access",
  delivery: "Deployment, testing and infrastructure reliability",
  data: "Data, schemas, APIs and migrations",
  scraping: "Scraping and external portals",
  documentation: "Documentation, guides and knowledge",
  other: "Other work",
} as const;
export type Theme = keyof typeof THEMES | "unclear";
export const THEME_VERSION = "clef-themes-v1";

export function themeInput(title: string) {
  // Titles are capped and redacted before leaving the hub. Do not send descriptions.
  return redactSensitiveText(title).slice(0, 500);
}
export function themeInputHash(title: string) {
  return createHash("sha256").update(THEME_VERSION).update("\0").update(themeInput(title)).digest("hex");
}

export function parseTheme(value: unknown): { theme: Theme; confidence: number } {
  const result = value as { success?: boolean; result?: { answers?: { theme?: { choice?: unknown; confidence?: unknown } } } };
  const answer = result?.result?.answers?.theme;
  if (!result?.success || typeof answer?.choice !== "string" || !Object.hasOwn(THEMES, answer.choice) || typeof answer.confidence !== "number" || !Number.isFinite(answer.confidence) || answer.confidence < 0 || answer.confidence > 1) throw new Error("Invalid Clef theme response");
  return { theme: answer.confidence < 0.5 ? "unclear" : answer.choice as Theme, confidence: answer.confidence };
}

export async function classifyTheme(title: string, config: { gatewayUrl: string; token: string; gatewayToken?: string; fetch?: typeof fetch }): Promise<{ theme: Theme; confidence: number }> {
  const url = new URL(config.gatewayUrl);
  if (url.protocol !== "https:" || url.hostname !== "gateway.ai.cloudflare.com" || !/^\/v1\/[^/]+\/[^/]+\/workers-ai\/@cf\/cloudflare\/clef$/.test(url.pathname) || url.search || url.hash || url.username || url.password) throw new Error("Invalid Clef gateway URL");
  const response = await (config.fetch ?? fetch)(url, {
    method: "POST", signal: AbortSignal.timeout(30_000),
    headers: { "Authorization": `Bearer ${config.token}`, ...(config.gatewayToken ? { "cf-aig-authorization": `Bearer ${config.gatewayToken}` } : {}), "Content-Type": "application/json", "cf-aig-skip-log": "true", "User-Agent": "Mozilla/5.0" },
    body: JSON.stringify({ model: "clef", state: themeInput(title), questions: { theme: { type: "choice", instructions: "Select the most relevant theme for this issue title. Use other if none fits.", criteria: THEMES } } }),
  });
  if (!response.ok) throw new Error(`Clef gateway failed (${response.status})`);
  return parseTheme(await response.json());
}
