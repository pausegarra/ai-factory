import { openConnector, applyConnector, cancelConnector } from "./fixtures/notification-editing";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { NotificationsPage } from "../src/context/factory/infrastructure/presentations/notifications";
import { newGoogleChatConnector, newTelegramConnector } from "../src/context/factory/domain/notification-connectors";
import { TRIGGERS, type Settings } from "../src/context/factory/domain/types";
import { noticeSettings, deferred } from "./fixtures/notices";
afterEach(cleanup);
const url="https://chat.googleapis.com/v1/spaces/fixture/messages?key=PRIVATE_KEY&token=PRIVATE_TOKEN";
function settings():Settings {return {...noticeSettings(),token_profiles:[{id:"work",label:"Work",account:"alice",has_token:true,owners:[],authors:[],reviewers:[]}]};}
function configured() {const c=newGoogleChatConnector("chat");c.enabled=true;c.config.has_webhook_url=true;c.config.token_profile_ids=["work"];return c;}
function save(){fireEvent.click(screen.getByRole("button",{name:"Save changes"}));}
it("opens a keyboard accessible provider selector without creating a connector or dirty state",async()=>{
  const user=userEvent.setup();const dirty=vi.fn();const onSave=vi.fn();
  render(<NotificationsPage settings={settings()} onSave={onSave} onDirtyChange={dirty}/>);
  const add=screen.getByRole("button",{name:"Add connector"});add.focus();await user.keyboard("{Enter}");
  const picker=screen.getByRole("dialog",{name:"Choose a connector"});
  expect(document.activeElement).toBe(within(picker).getByRole("button",{name:"Close connector selector"}));
  expect(within(picker).getByRole("button",{name:"Telegram"})).toBeTruthy();
  expect(within(picker).getByRole("button",{name:"Google Chat"})).toBeTruthy();
  expect(screen.queryByLabelText("Webhook URL")).toBeNull();expect(screen.queryByLabelText("Bot token")).toBeNull();
  expect(dirty).toHaveBeenLastCalledWith(false);
  await user.keyboard("{Escape}");expect(screen.queryByRole("dialog")).toBeNull();expect(document.activeElement).toBe(add);expect(onSave).not.toHaveBeenCalled();
});
it("adds Google Chat disabled with only URL credentials and saves filters while clearing returned secrets",async()=>{
  const user=userEvent.setup();const onSave=vi.fn(async(s:Settings)=>s);
  render(<NotificationsPage settings={settings()} onSave={onSave}/>);
  await user.click(screen.getByRole("button",{name:"Add connector"}));await user.click(within(screen.getByRole("dialog",{name:"Choose a connector"})).getByRole("button",{name:"Google Chat"}));
  const card=screen.getByRole("dialog",{name:"Configure new connector"});expect(document.activeElement).toBe(within(card).getByRole("button",{name:"Close connector editor"}));
  const input=screen.getByLabelText("Webhook URL") as HTMLInputElement;expect(input.type).toBe("password");
  expect(screen.queryByLabelText("Bot token")).toBeNull();expect((screen.getByRole("checkbox",{name:"Enabled"}) as HTMLInputElement).checked).toBe(false);
  expect(within(screen.getByRole("group",{name:"GitHub triggers"})).getAllByRole("checkbox").map(c=>c.getAttribute("aria-label") ?? c.closest("label")?.textContent)).toHaveLength(4);
  for (const t of Object.values(TRIGGERS)) expect(screen.getByRole("checkbox",{name:t.label})).toBeTruthy();
  await user.type(input,url);await user.click(screen.getByRole("checkbox",{name:"Work (@alice)"}));await user.click(screen.getByRole("checkbox",{name:"Enabled"}));
  await user.click(screen.getByRole("checkbox",{name:"PR approved"}));applyConnector();save();await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config).toEqual({has_webhook_url:false,webhook_url:url,triggers:["new_issue","pr_review","release"],token_profile_ids:["work"]});
  openConnector("Google Chat"); expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe("");expect(screen.getByText("Webhook configured")).toBeTruthy();
});
it("preserves a configured webhook with blank input and replaces it only on explicit submission",async()=>{
  const onSave=vi.fn(async(_s:Settings)=>{});render(<NotificationsPage settings={{...settings(),notification_connectors:[configured()]}} onSave={onSave}/>);
  openConnector("Google Chat");
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}});applyConnector();save();await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.webhook_url).toBeUndefined(); openConnector("Renamed");
  fireEvent.change(screen.getByLabelText("Webhook URL"),{target:{value:`  ${url}  `}});applyConnector();save();await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
  expect(onSave.mock.calls[1][0].notification_connectors?.[0].config.webhook_url).toBe(url); openConnector("Renamed");
  await waitFor(()=>expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe(""));
});
it("allows incomplete disabled drafts but associates all enabled requirement errors",async()=>{
  const c=newGoogleChatConnector("chat");c.events=[];c.config.triggers=[];const onSave=vi.fn(async(s:Settings)=>s);
  render(<NotificationsPage settings={{...settings(),notification_connectors:[c]}} onSave={onSave}/>);
  openConnector("Google Chat");
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Draft"}});applyConnector();save();await screen.findByText("Notification settings saved.");
  openConnector("Draft"); fireEvent.click(screen.getByRole("checkbox",{name:"Enabled"}));applyConnector();save();expect(onSave).toHaveBeenCalledTimes(1);
  for (const name of ["Run events","GitHub triggers","GitHub profiles"]) {const group=screen.getByRole("group",{name});expect(group.getAttribute("aria-invalid")).toBe("true");expect(document.getElementById(group.getAttribute("aria-describedby")!)?.textContent).toBeTruthy();}
  const input=screen.getByLabelText("Webhook URL");expect(input.getAttribute("aria-invalid")).toBe("true");expect(document.getElementById(input.getAttribute("aria-describedby")!)?.textContent).toContain("Webhook URL");
});
it("rejects unsafe URL drafts even when disabled without echoing the credential",()=>{
  const c=newGoogleChatConnector("chat");const onSave=vi.fn();render(<NotificationsPage settings={{...settings(),notification_connectors:[c]}} onSave={onSave}/>);
  openConnector("Google Chat");
  for(const value of ["http://host/PRIVATE_TOKEN","https://u:PRIVATE_TOKEN@host/path","https://host/path#PRIVATE_TOKEN","https://host/a b"]){
    fireEvent.change(screen.getByLabelText("Webhook URL"),{target:{value}});applyConnector();save();expect(onSave).not.toHaveBeenCalled();expect(within(screen.queryByRole("dialog") ?? document.body).getByRole("alert").textContent).not.toContain("PRIVATE_TOKEN");expect(screen.getByLabelText("Webhook URL").getAttribute("aria-invalid")).toBe("true");
  }
});
it("preserves IDs through profile refreshes and exposes removed selections without authorizing new profiles",async()=>{
  const original={...settings(),notification_connectors:[configured()]};const onSave=vi.fn(async(s:Settings)=>s);
  const view=render(<NotificationsPage settings={original} onSave={onSave}/>);
  openConnector("Google Chat");
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Dirty"}});
  const added={id:"new",label:"New",account:"bob",has_token:true,owners:[],authors:[],reviewers:[]};
  view.rerender(<NotificationsPage settings={{...original,token_profiles:[{...original.token_profiles[0],label:"Renamed"},added]}} onSave={onSave}/>);
  expect((screen.getByRole("checkbox",{name:"Renamed (@alice)"}) as HTMLInputElement).checked).toBe(true);expect((screen.getByRole("checkbox",{name:"New (@bob)"}) as HTMLInputElement).checked).toBe(false);
  view.rerender(<NotificationsPage settings={{...original,token_profiles:[added]}} onSave={onSave}/>);
  const missing=screen.getByRole("checkbox",{name:"Unavailable profile (work)"});expect((missing as HTMLInputElement).checked).toBe(true);applyConnector();save();expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox",{name:"Unavailable profile (work)"}));fireEvent.click(screen.getByRole("checkbox",{name:"New (@bob)"}));applyConnector();save();await screen.findByText("Notification settings saved.");expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.token_profile_ids).toEqual(["new"]);
});
it("uses refreshed webhook presence without overwriting a dirty URL and keeps credentials out of notices",async()=>{
  const original={...settings(),notification_connectors:[configured()]};const pending=deferred<Settings>();const onSave=vi.fn(()=>pending.promise);
  const view=render(<NotificationsPage settings={original} onSave={onSave}/>);
  openConnector("Google Chat");
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Dirty"}});
  const absent=configured();absent.config.has_webhook_url=false;
  view.rerender(<NotificationsPage settings={{...original,notification_connectors:[absent]}} onSave={onSave}/>);applyConnector();save();expect(onSave).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Webhook URL"),{target:{value:url}});applyConnector();save();
  expect(screen.getByRole("button",{name:"Add connector"}).matches(":disabled")).toBe(true);expect(screen.getByRole("button",{name:"Edit connector Dirty"}).matches(":disabled")).toBe(true); expect(screen.queryByLabelText("Webhook URL")).toBeNull();
  pending.reject(new Error(url));await screen.findByRole("alert");expect(within(screen.queryByRole("dialog") ?? document.body).getByRole("alert").textContent).not.toContain("PRIVATE_TOKEN");openConnector("Dirty");expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe(url);
  cancelConnector();fireEvent.click(screen.getByRole("button",{name:"Discard changes"}));openConnector("Google Chat");expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe("");
});
it("isolates multiple Google Chat connectors from Telegram and restores focus after final removal",async()=>{
  const a=newGoogleChatConnector("a"),b=newGoogleChatConnector("b");b.label="Second";
  const onSave=vi.fn(async(s:Settings)=>s);const user=userEvent.setup();
  render(<NotificationsPage settings={{...settings(),notification_connectors:[a,b,newTelegramConnector("tg")]}} onSave={onSave}/>);
  openConnector("Google Chat");
  await user.click(screen.getByRole("checkbox",{name:"Run started"})); applyConnector();
  openConnector("Second"); expect((screen.getByRole("checkbox",{name:"Run started"}) as HTMLInputElement).checked).toBe(true); cancelConnector();
  openConnector("Google Chat"); await user.click(screen.getByRole("button",{name:"Remove connector Google Chat"}));
  await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Edit connector Second"})));
  save(); await screen.findByText("Notification settings saved."); expect(onSave.mock.calls[0][0].notification_connectors?.map(c=>c.id)).toEqual(["b","tg"]);
  openConnector("Second"); await user.click(screen.getByRole("button",{name:"Remove connector Second"}));
  openConnector("Telegram"); await user.click(screen.getByRole("button",{name:"Remove connector Telegram"}));
  await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
});
it("explains missing profiles and never renders GitHub token values",()=>{
  const c=newGoogleChatConnector("chat");const s=settings();s.token_profiles[0].token="PRIVATE_GITHUB";
  const view=render(<NotificationsPage settings={{...s,notification_connectors:[c]}} onSave={vi.fn()}/>);openConnector("Google Chat");expect(document.body.innerHTML).not.toContain("PRIVATE_GITHUB");
  view.rerender(<NotificationsPage settings={{...s,token_profiles:[],notification_connectors:[c]}} onSave={vi.fn()}/>);expect(screen.getByText("Add a GitHub profile in Settings to enable this connector.")).toBeTruthy();
});
