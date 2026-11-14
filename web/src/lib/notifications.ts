import { managementRequest } from "./management-api";
export const providers = ["telegram", "discord", "slack", "webhook", "ntfy", "gotify", "apprise", "pushover"] as const;
export type Provider = typeof providers[number];
export const providerNames: Record<Provider, string> = { telegram: "Telegram", discord: "Discord", slack: "Slack", webhook: "Webhook", ntfy: "ntfy", gotify: "Gotify", apprise: "Apprise API", pushover: "Pushover" };
export const secretFields: Record<Provider, string[]> = {
  telegram: ["token"], discord: ["url"], slack: ["url"], webhook: ["url", "bearer"], ntfy: ["token"], gotify: ["token"], apprise: ["url", "key"], pushover: ["token", "user"],
};
export type NotificationConfig = { apiBase?: string; chatId?: string; threadId?: number; disableNotification?: boolean; topic?: string; priority?: number; allowInsecureLocal?: boolean };
export type Channel = { id: string; name: string; provider: Provider; enabled: boolean; config: NotificationConfig; events: string[]; storedSecrets: string[]; revision: number };
export type ChannelDraft = Omit<Channel, "id" | "storedSecrets"> & { secrets: Record<string, string | null> };
export type NotificationEvent = { id: string; title: string; message: string };
export type NotificationTemplate = { scope: string; event: string; title: string; body: string };
export type Delivery = { id: string; channelId: string; event: string; status: string; attempts: number; summary: string | null; durationMs: number | null; isTest: boolean; createdAt: number; nextAttempt: number };
export type TestResult = { success: boolean; status: string; summary: string; durationMs: number };
export function draftChannel(c: Channel): ChannelDraft { return { name: c.name, provider: c.provider, enabled: c.enabled, config: { ...c.config }, events: [...c.events], secrets: {}, revision: c.revision }; }
export function newChannel(provider: Provider = "telegram"): ChannelDraft { return { name: "", provider, enabled: true, config: {}, events: ["system.startup", "automation.failed", "indexer.test.failed"], secrets: {}, revision: 0 }; }
export async function notificationApi<T>(token: string, path: string, init?: RequestInit): Promise<T> {
  return (await managementRequest(token, "notifications/" + path, { ...init, headers: { "Content-Type": "application/json", ...init?.headers } })).json() as Promise<T>;
}
