import type { NotificationConnector, NotificationEvent, TelegramNotificationConnector } from "./types";
export const notificationEvents: {id: NotificationEvent; label: string}[] = [
  {id: "run_started", label: "Run started"},
  {id: "run_succeeded", label: "Run succeeded"},
  {id: "run_failed", label: "Run failed"},
  {id: "run_blocked", label: "Run blocked"},
];
export function newTelegramConnector(id: string): TelegramNotificationConnector {
  return {id, label: "Telegram", enabled: false, events: notificationEvents.map(e => e.id), type: "telegram", config: {chat_id: "", has_token: false}};
}
export function isTelegramConnector(c: NotificationConnector): c is TelegramNotificationConnector {
  return c.type === "telegram" && typeof c.config.chat_id === "string" && typeof c.config.has_token === "boolean";
}
export function validateNotificationConnectors(connectors: NotificationConnector[]): string[] {
  const errors: string[] = [];
  const ids = new Set<string>();
  connectors.forEach((c, index) => {
    const prefix = `Connector ${index + 1}: `;
    if (!/^[a-zA-Z0-9_.-]{1,128}$/.test(c.id) || c.id === "." || c.id === ".." || ids.has(c.id)) errors.push(prefix + "IDs must be valid and unique.");
    ids.add(c.id);
    if (!c.label.trim()) errors.push(prefix + "Name is required.");
    if (new Set(c.events).size !== c.events.length || c.events.some(e => !notificationEvents.some(option => option.id === e))) errors.push(prefix + "Invalid run events.");
    if (c.type === "telegram") {
      if (!isTelegramConnector(c)) { errors.push(prefix + "Invalid Telegram configuration."); return; }
      if (c.enabled) {
        if (!c.config.chat_id.trim()) errors.push(prefix + "Chat ID is required.");
        if (!c.config.has_token && !c.config.bot_token?.trim()) errors.push(prefix + "Bot token is required.");
        if (!c.events.length) errors.push(prefix + "Select at least one run event.");
      }
    }
  });
  return errors;
}
export function cleanNotificationConnectors(connectors: NotificationConnector[]): NotificationConnector[] {
  return structuredClone(connectors).map(c => {
    if (c.type === "telegram") delete c.config.bot_token;
    return c;
  });
}
