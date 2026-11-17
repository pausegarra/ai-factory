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
  a.config.bot_token = "fixture"; a.config.token_profile_ids = ["work"];
  expect(validateNotificationConnectors([a],filterProfiles)).toEqual([]);
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
  const c=newTelegramConnector("team.eu");c.enabled=true;c.config.has_token=true;c.config.token_profile_ids=["work"];
  for (const chat of ["-1001234567","@channel_name","textual destination"]) {
    c.config.chat_id=chat;expect(validateNotificationConnectors([c],filterProfiles)).toEqual([]);
  }
});
it("cleans Google Chat URL drafts without mutating the editor", () => {
  const c = {id:"gchat",label:"Team",enabled:false,events:[],type:"google_chat",config:{has_webhook_url:false,webhook_url:"PRIVATE_DRAFT",triggers:["new_issue"],token_profile_ids:[]}};
  expect(cleanNotificationConnectors([c])[0].config.webhook_url).toBeUndefined();
  expect(c.config.webhook_url).toBe("PRIVATE_DRAFT");
});
it("requires every Google Chat filter and a real configured or new webhook", () => {
  const c = {id:"gchat",label:"Team",enabled:true,events:[],type:"google_chat",config:{has_webhook_url:false,triggers:[],token_profile_ids:[]}};
  const errors=validateNotificationConnectors([c]).join(" ");
  for (const word of ["webhook", "event", "trigger", "profile"]) expect(errors.toLowerCase()).toContain(word);
});
it("uses independent Google Chat defaults and explicitly selected stable profiles", async () => {
  const {newGoogleChatConnector}=await import("../src/context/factory/domain/notification-connectors");
  const a=newGoogleChatConnector("a"),b=newGoogleChatConnector("b");
  expect(a.enabled).toBe(false);expect(a.config.token_profile_ids).toEqual([]);expect(a.config.triggers).toEqual(["new_issue","pr_approved","pr_review","release"]);
  a.config.triggers.pop();expect(b.config.triggers).toHaveLength(4);
  a.config.webhook_url="PRIVATE_DRAFT";expect(cleanNotificationConnectors([a])[0].config.webhook_url).toBeUndefined();
});
it("rejects parser-normalized unsafe webhooks and validates filter IDs",async()=>{
  const {validWebhookUrl,newGoogleChatConnector}=await import("../src/context/factory/domain/notification-connectors");
  for(const value of ["http://host/path","https:///path","https://u:p@host/path","https://@host/path","https://host/path#","https://host/a b","https://host/a\\b","https://host/a\n"]) expect(validWebhookUrl(value)).toBe(false);
  expect(validWebhookUrl("  https://alternate.example/custom?key=fixture&token=fixture&extra=keep  ")).toBe(true);
  const c=newGoogleChatConnector("chat");c.enabled=true;c.config.has_webhook_url=true;c.config.token_profile_ids=["work"];
  const profiles=[{id:"work",label:"Renamed",account:"alice",has_token:true,owners:[],authors:[],reviewers:[]}];
  expect(validateNotificationConnectors([c],profiles)).toEqual([]);
  for(const ids of [["work","work"],["../work"],["deleted"]]) {c.config.token_profile_ids=ids;expect(validateNotificationConnectors([c],profiles).join(" ").toLowerCase()).toContain("profile");}
});
it("rejects malformed Google Chat configuration without throwing or reflecting values",()=>{
  for(const config of [
    {has_webhook_url:false,webhook_url:17,triggers:[],token_profile_ids:[]},
    {has_webhook_url:false,triggers:[null],token_profile_ids:[]},
    {has_webhook_url:false,triggers:[],token_profile_ids:[17]},
  ]) {
    expect(validateNotificationConnectors([{id:"chat",label:"Chat",enabled:false,events:[],type:"google_chat",config}])).toEqual(["Connector 1: Invalid Google Chat configuration."]);
  }
});

it("gives Telegram the common filters without sharing selections", () => {
  const a = newTelegramConnector("a"), b = newTelegramConnector("b");
  expect(a.enabled).toBe(false);
  expect(a.config.triggers).toEqual(["new_issue", "pr_approved", "pr_review", "release"]);
  expect(a.config.token_profile_ids).toEqual([]);
  a.config.triggers.pop(); a.config.token_profile_ids.push("work");
  expect(b.config.triggers).toEqual(["new_issue", "pr_approved", "pr_review", "release"]);
  expect(b.config.token_profile_ids).toEqual([]);
});

import { newGoogleChatConnector } from "../src/context/factory/domain/notification-connectors";
import type { NotificationConnector } from "../src/context/factory/domain/types";
const filterProfiles = [{id:"work", label:"Work", account:"alice", has_token:true, owners:[], authors:[], reviewers:[]}];
for (const create of [newTelegramConnector, newGoogleChatConnector]) {
  it(`validates common filters for ${create("fixture").type} independently of credentials`, () => {
    const c = create("fixture");
    c.enabled = true;
    c.config = {...c.config, chat_id:"fixture", has_token:true, has_webhook_url:true, token_profile_ids:["work"]};
    expect(validateNotificationConnectors([c],filterProfiles)).toEqual([]);
    for (const [field, value, message, structural] of [
      ["events",[],"Select at least one run event.",false],
      ["events",["run_started","run_started"],"Invalid run events.",true],
      ["events",["PRIVATE_SENTINEL"],"Invalid run events.",true],
      ["triggers",[],"Select at least one GitHub trigger.",false],
      ["triggers",["new_issue","new_issue"],"Invalid GitHub triggers.",true],
      ["triggers",["toString"],"Invalid GitHub triggers.",true],
      ["triggers",["PRIVATE_SENTINEL"],"Invalid GitHub triggers.",true],
      ["token_profile_ids",[],"Select available GitHub profiles.",false],
      ["token_profile_ids",["deleted"],"Select available GitHub profiles.",false],
      ...[["work","work"],["../work"],["."],[".."],["x".repeat(129)]].map(value => ["token_profile_ids",value,"Invalid GitHub profiles.",true] as const),
    ] as const) {
      for (const enabled of [true,false]) {
        const edited: NotificationConnector = structuredClone(c);
        edited.enabled = enabled;
        if(field === "events") Object.assign(edited,{events:value});
        else edited.config[field] = value;
        const errors = validateNotificationConnectors([edited],filterProfiles);
        if(enabled || structural) expect(errors.join(" ")).toContain(message);
        else expect(errors).toEqual([]);
        expect(errors.join(" ")).not.toContain("PRIVATE_SENTINEL");
      }
    }
    expect(validateNotificationConnectors([c],[{...filterProfiles[0],label:"Renamed"}])).toEqual([]);
    expect(c.config.token_profile_ids).toEqual(["work"]);
    const clean = cleanNotificationConnectors([{...c,config:{...c.config, bot_token:"PRIVATE_DRAFT",webhook_url:"PRIVATE_DRAFT"}}]);
    expect(clean[0].config.triggers).toEqual(["new_issue","pr_approved","pr_review","release"]);
    expect(clean[0].config.token_profile_ids).toEqual(["work"]);
  });
  it(`rejects malformed filter shapes for ${create("fixture").type} safely`, () => {
    for(const field of ["triggers","token_profile_ids",create("fixture").type === "telegram" ? "bot_token" : "webhook_url"]) {
      for(const value of [null,17,{PRIVATE_SENTINEL:1},[17]]) {
        const c: NotificationConnector = create("fixture"); c.config[field]=value;
        const errors = validateNotificationConnectors([c]);
        expect(errors.join(" ")).toContain("configuration");
        expect(errors.join(" ")).not.toContain("PRIVATE_SENTINEL");
      }
    }
  });
}

it.each([newTelegramConnector,newGoogleChatConnector])("includes Release in new defaults and preserves explicit selections",create=>{
  const c=create("release-default");expect(c.config.triggers).toEqual(["new_issue","pr_approved","pr_review","release"]);
  const existing={...c,config:{...c.config,triggers:["new_issue"]}};
  expect(cleanNotificationConnectors([existing])[0].config.triggers).toEqual(["new_issue"]);
});
