import { it, expect } from "vitest";
import { newTelegramConnector, validateNotificationConnectors, cleanNotificationConnectors } from "../src/context/factory/domain/notification-connectors";
it("starts disabled with independent event preferences", () => {
  const a = newTelegramConnector("first");
  const b = newTelegramConnector("second");
  expect(a.enabled).toBe(false);
  expect(a.events).toEqual(["run_started", "run_succeeded", "run_failed", "run_blocked"]);
  a.events.pop();
  expect(b.events).toHaveLength(4);
  expect(validateNotificationConnectors([b])).toEqual([]);
});
it("validates active requirements, duplicates and identity without echoing secrets", () => {
  const a = newTelegramConnector("team"); a.enabled = true;
  expect(validateNotificationConnectors([a]).join(" ")).toContain("Chat ID");
  a.config.chat_id = "-1001";
  expect(validateNotificationConnectors([a]).join(" ")).toContain("Bot token");
  a.config.bot_token = "fixture";
  expect(validateNotificationConnectors([a])).toEqual([]);
  a.events = [];
  expect(validateNotificationConnectors([a]).join(" ")).toContain("event");
  a.enabled = false;
  expect(validateNotificationConnectors([a, structuredClone(a)]).join(" ")).toContain("unique");
});
it("cleans token drafts deeply while preserving unknown provider settings", () => {
  const a = newTelegramConnector("a"); a.config.bot_token = "PRIVATE_DRAFT";
  const future = {id:"future",label:"Future",enabled:true,events:[],type:"future",config:{nested:{value:7}}};
  const clean = cleanNotificationConnectors([a, future]);
  expect(clean[0].config.bot_token).toBeUndefined();
  expect(a.config.bot_token).toBe("PRIVATE_DRAFT");
  expect(clean[1]).toEqual(future);
});
it("accepts existing backend identities and textual destinations", () => {
  const c=newTelegramConnector("team.eu");c.enabled=true;c.config.has_token=true;
  for (const chat of ["-1001234567","@channel_name","textual destination"]) {
    c.config.chat_id=chat;expect(validateNotificationConnectors([c])).toEqual([]);
  }
});
