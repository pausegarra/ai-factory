import type { GitHubNotificationFilters, NotificationConnector, NotificationEvent, TelegramNotificationConnector, GoogleChatNotificationConnector, SourceConnector, Trigger } from "./types";
import { TRIGGERS } from "./types";
export const notificationEvents: {id: NotificationEvent; label: string}[] = [
  {id: "run_started", label: "Run started"},
  {id: "run_succeeded", label: "Run succeeded"},
  {id: "run_failed", label: "Run failed"},
  {id: "run_blocked", label: "Run blocked"},
];
function newGitHubNotificationFilters(): GitHubNotificationFilters {
  return {triggers: Object.keys(TRIGGERS) as Trigger[], token_profile_ids: []};
}
function hasFilterShape(config: Record<string, unknown>): boolean {
  return Array.isArray(config.triggers) && config.triggers.every(t => typeof t === "string")
    && Array.isArray(config.token_profile_ids) && config.token_profile_ids.every(id => typeof id === "string");
}
export function newTelegramConnector(id: string): TelegramNotificationConnector {
  return {id, label: "Telegram", enabled: false, events: notificationEvents.map(e => e.id), type: "telegram", config: {chat_id: "", has_token: false, ...newGitHubNotificationFilters()}};
}
export function isTelegramConnector(c: NotificationConnector): c is TelegramNotificationConnector {
  return c.type === "telegram" && typeof c.config.chat_id === "string" && typeof c.config.has_token === "boolean"
    && (c.config.bot_token === undefined || typeof c.config.bot_token === "string") && hasFilterShape(c.config);
}
export function newGoogleChatConnector(id: string): GoogleChatNotificationConnector {
  return {id, label: "Google Chat", enabled: false, events: notificationEvents.map(e => e.id), type: "google_chat", config: {has_webhook_url: false, ...newGitHubNotificationFilters()}};
}
export function isGoogleChatConnector(c: NotificationConnector): c is GoogleChatNotificationConnector {
  return c.type === "google_chat" && typeof c.config.has_webhook_url === "boolean"
    && (c.config.webhook_url === undefined || typeof c.config.webhook_url === "string")
    && hasFilterShape(c.config);
}
export function validWebhookUrl(raw: string): boolean {
  const value = raw.trim();
  if (!value || /[\u0000-\u001f\u007f]/u.test(raw) || /[\s\\]/u.test(value)) return false;
  const authority = value.split("://")[1]?.split(/[/?#]/)[0];
  if (!authority || authority.includes("@")) return false;
  try {
    const url = new URL(value);
    return /^https:\/\//i.test(value) && url.protocol === "https:" && !!url.hostname && !url.username && !url.password && !value.includes("#") && !value.slice(value.indexOf("://") + 3).split(/[/?]/)[0].includes("@");
  } catch { return false; }
}
function validateFilters(c: TelegramNotificationConnector | GoogleChatNotificationConnector, sources: SourceConnector[], prefix: string, errors: string[]): void {
  if (new Set(c.config.triggers).size !== c.config.triggers.length || c.config.triggers.some(t => !Object.prototype.hasOwnProperty.call(TRIGGERS, t))) errors.push(prefix + "Invalid GitHub triggers.");
  if (new Set(c.config.token_profile_ids).size !== c.config.token_profile_ids.length || c.config.token_profile_ids.some(id => !/^[a-zA-Z0-9_.-]{1,128}$/.test(id) || id === "." || id === "..")) errors.push(prefix + "Invalid GitHub sources.");
  if (c.enabled) {
    if (!c.events.length) errors.push(prefix + "Select at least one run event.");
    if (!c.config.triggers.length) errors.push(prefix + "Select at least one GitHub trigger.");
    if (!c.config.token_profile_ids.length || c.config.token_profile_ids.some(id => !sources.some(p => p.id === id))) errors.push(prefix + "Select available GitHub sources.");
  }
}
export function validateNotificationConnectors(connectors: NotificationConnector[], sources: SourceConnector[] = []): string[] {
  const errors: string[] = [];
  const ids = new Set<string>();
  connectors.forEach((c, index) => {
    const prefix = `Connector ${index + 1}: `;
    if (!/^[a-zA-Z0-9_.-]{1,128}$/.test(c.id) || c.id === "." || c.id === ".." || ids.has(c.id)) errors.push(prefix + "IDs must be valid and unique.");
    ids.add(c.id);
    if (!c.label.trim()) errors.push(prefix + "Name is required.");
    if (new Set(c.events).size !== c.events.length || c.events.some(e => !notificationEvents.some(option => option.id === e))) errors.push(prefix + "Invalid run events.");
    if (c.type === "google_chat") {
      if (!isGoogleChatConnector(c)) { errors.push(prefix + "Invalid Google Chat configuration."); return; }
      const url = c.config.webhook_url?.trim();
      if (url && !validWebhookUrl(c.config.webhook_url ?? "")) errors.push(prefix + "Enter a valid HTTPS webhook URL without credentials or a fragment.");
      validateFilters(c, sources, prefix, errors);
      if (c.enabled && !url && !c.config.has_webhook_url) errors.push(prefix + "Webhook URL is required.");
    }
    if (c.type === "telegram") {
      if (!isTelegramConnector(c)) { errors.push(prefix + "Invalid Telegram configuration."); return; }
      if (c.enabled) {
        if (!c.config.chat_id.trim()) errors.push(prefix + "Chat ID is required.");
        if (!c.config.has_token && !c.config.bot_token?.trim()) errors.push(prefix + "Bot token is required.");
      }
      validateFilters(c, sources, prefix, errors);
    }
  });
  return errors;
}
export function cleanNotificationConnectors(connectors: NotificationConnector[]): NotificationConnector[] {
  return structuredClone(connectors).map(c => {
    if (c.type === "telegram") delete c.config.bot_token;
    if (c.type === "google_chat") delete c.config.webhook_url;
    return c;
  });
}
