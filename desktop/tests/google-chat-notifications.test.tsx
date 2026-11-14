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
  const picker=screen.getByRole("combobox",{name:"Connector to add"});expect(document.activeElement).toBe(picker);
  expect(within(picker).getAllByRole("option").map(o=>(o as HTMLOptionElement).value)).toEqual(["","telegram","google_chat"]);
  expect(screen.queryByLabelText("Webhook URL")).toBeNull();expect(screen.queryByLabelText("Bot token")).toBeNull();
  expect(dirty).toHaveBeenLastCalledWith(false);
  await user.keyboard("{Escape}");expect(screen.queryByRole("combobox",{name:"Connector to add"})).toBeNull();expect(document.activeElement).toBe(add);expect(onSave).not.toHaveBeenCalled();
});
it("adds Google Chat disabled with only URL credentials and saves filters while clearing returned secrets",async()=>{
  const user=userEvent.setup();const onSave=vi.fn(async(s:Settings)=>s);
  render(<NotificationsPage settings={settings()} onSave={onSave}/>);
  await user.click(screen.getByRole("button",{name:"Add connector"}));await user.selectOptions(screen.getByRole("combobox",{name:"Connector to add"}),"google_chat");
  const card=screen.getByRole("group",{name:"Google Chat"});expect(document.activeElement).toBe(card);
  const input=screen.getByLabelText("Webhook URL") as HTMLInputElement;expect(input.type).toBe("password");
  expect(screen.queryByLabelText("Bot token")).toBeNull();expect((screen.getByRole("checkbox",{name:"Enabled"}) as HTMLInputElement).checked).toBe(false);
  expect(within(screen.getByRole("group",{name:"GitHub triggers"})).getAllByRole("checkbox").map(c=>c.getAttribute("aria-label") ?? c.closest("label")?.textContent)).toHaveLength(3);
  for (const t of Object.values(TRIGGERS)) expect(screen.getByRole("checkbox",{name:t.label})).toBeTruthy();
  await user.type(input,url);await user.click(screen.getByRole("checkbox",{name:"Work (@alice)"}));await user.click(screen.getByRole("checkbox",{name:"Enabled"}));
  await user.click(screen.getByRole("checkbox",{name:"PR approved"}));save();await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config).toEqual({has_webhook_url:false,webhook_url:url,triggers:["new_issue","pr_review"],token_profile_ids:["work"]});
  expect(input.value).toBe("");expect(screen.getByText("Webhook configured")).toBeTruthy();
});
it("preserves a configured webhook with blank input and replaces it only on explicit submission",async()=>{
  const onSave=vi.fn(async(_s:Settings)=>{});render(<NotificationsPage settings={{...settings(),notification_connectors:[configured()]}} onSave={onSave}/>);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}});save();await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.webhook_url).toBeUndefined();
  fireEvent.change(screen.getByLabelText("Webhook URL"),{target:{value:`  ${url}  `}});save();await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
  expect(onSave.mock.calls[1][0].notification_connectors?.[0].config.webhook_url).toBe(url);
  await waitFor(()=>expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe(""));
});
it("allows incomplete disabled drafts but associates all enabled requirement errors",async()=>{
  const c=newGoogleChatConnector("chat");c.events=[];c.config.triggers=[];const onSave=vi.fn(async(s:Settings)=>s);
  render(<NotificationsPage settings={{...settings(),notification_connectors:[c]}} onSave={onSave}/>);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Draft"}});save();await screen.findByText("Notification settings saved.");
  fireEvent.click(screen.getByRole("checkbox",{name:"Enabled"}));save();expect(onSave).toHaveBeenCalledTimes(1);
  for (const name of ["Run events","GitHub triggers","GitHub profiles"]) {const group=screen.getByRole("group",{name});expect(group.getAttribute("aria-invalid")).toBe("true");expect(document.getElementById(group.getAttribute("aria-describedby")!)?.textContent).toBeTruthy();}
  const input=screen.getByLabelText("Webhook URL");expect(input.getAttribute("aria-invalid")).toBe("true");expect(document.getElementById(input.getAttribute("aria-describedby")!)?.textContent).toContain("Webhook URL");
});
it("rejects unsafe URL drafts even when disabled without echoing the credential",()=>{
  const c=newGoogleChatConnector("chat");const onSave=vi.fn();render(<NotificationsPage settings={{...settings(),notification_connectors:[c]}} onSave={onSave}/>);
  for(const value of ["http://host/PRIVATE_TOKEN","https://u:PRIVATE_TOKEN@host/path","https://host/path#PRIVATE_TOKEN","https://host/a b"]){
    fireEvent.change(screen.getByLabelText("Webhook URL"),{target:{value}});save();expect(onSave).not.toHaveBeenCalled();expect(screen.getByRole("alert").textContent).not.toContain("PRIVATE_TOKEN");expect(screen.getByLabelText("Webhook URL").getAttribute("aria-invalid")).toBe("true");
  }
});
it("preserves IDs through profile refreshes and exposes removed selections without authorizing new profiles",async()=>{
  const original={...settings(),notification_connectors:[configured()]};const onSave=vi.fn(async(s:Settings)=>s);
  const view=render(<NotificationsPage settings={original} onSave={onSave}/>);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Dirty"}});
  const added={id:"new",label:"New",account:"bob",has_token:true,owners:[],authors:[],reviewers:[]};
  view.rerender(<NotificationsPage settings={{...original,token_profiles:[{...original.token_profiles[0],label:"Renamed"},added]}} onSave={onSave}/>);
  expect((screen.getByRole("checkbox",{name:"Renamed (@alice)"}) as HTMLInputElement).checked).toBe(true);expect((screen.getByRole("checkbox",{name:"New (@bob)"}) as HTMLInputElement).checked).toBe(false);
  view.rerender(<NotificationsPage settings={{...original,token_profiles:[added]}} onSave={onSave}/>);
  const missing=screen.getByRole("checkbox",{name:"Unavailable profile (work)"});expect((missing as HTMLInputElement).checked).toBe(true);save();expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(missing);fireEvent.click(screen.getByRole("checkbox",{name:"New (@bob)"}));save();await screen.findByText("Notification settings saved.");expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.token_profile_ids).toEqual(["new"]);
});
it("uses refreshed webhook presence without overwriting a dirty URL and keeps credentials out of notices",async()=>{
  const original={...settings(),notification_connectors:[configured()]};const pending=deferred<Settings>();const onSave=vi.fn(()=>pending.promise);
  const view=render(<NotificationsPage settings={original} onSave={onSave}/>);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Dirty"}});
  const absent=configured();absent.config.has_webhook_url=false;
  view.rerender(<NotificationsPage settings={{...original,notification_connectors:[absent]}} onSave={onSave}/>);save();expect(onSave).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Webhook URL"),{target:{value:url}});save();
  expect(screen.getByRole("button",{name:"Add connector"}).matches(":disabled")).toBe(true);expect(screen.getByLabelText("Webhook URL").matches(":disabled")).toBe(true);
  pending.reject(new Error(url));await screen.findByRole("alert");expect(screen.getByRole("alert").textContent).not.toContain("PRIVATE_TOKEN");expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe(url);
  fireEvent.click(screen.getByRole("button",{name:"Discard changes"}));expect((screen.getByLabelText("Webhook URL") as HTMLInputElement).value).toBe("");
});
it("isolates multiple Google Chat connectors from Telegram and restores focus after final removal",async()=>{
  const a=newGoogleChatConnector("a"),b=newGoogleChatConnector("b");b.label="Second";
  const onSave=vi.fn(async(s:Settings)=>s);const user=userEvent.setup();
  render(<NotificationsPage settings={{...settings(),notification_connectors:[a,b,newTelegramConnector("tg")]}} onSave={onSave}/>);
  const first=within(screen.getByRole("group",{name:"Google Chat"}));const second=within(screen.getByRole("group",{name:"Second"}));
  await user.click(first.getByRole("checkbox",{name:"Run started"}));expect((second.getByRole("checkbox",{name:"Run started"}) as HTMLInputElement).checked).toBe(true);
  await user.click(first.getByRole("button",{name:"Remove connector Google Chat"}));save();await screen.findByText("Notification settings saved.");expect(onSave.mock.calls[0][0].notification_connectors?.map(c=>c.id)).toEqual(["b","tg"]);
  await user.click(screen.getByRole("button",{name:"Remove connector Second"}));await user.click(screen.getByRole("button",{name:"Remove connector Telegram"}));await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
});
it("explains missing profiles and never renders GitHub token values",()=>{
  const c=newGoogleChatConnector("chat");const s=settings();s.token_profiles[0].token="PRIVATE_GITHUB";
  const view=render(<NotificationsPage settings={{...s,notification_connectors:[c]}} onSave={vi.fn()}/>);expect(document.body.innerHTML).not.toContain("PRIVATE_GITHUB");
  view.rerender(<NotificationsPage settings={{...s,token_profiles:[],notification_connectors:[c]}} onSave={vi.fn()}/>);expect(screen.getByText("Add a GitHub profile in Settings to enable this connector.")).toBeTruthy();
});
