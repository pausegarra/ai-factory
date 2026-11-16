import { openConnector, chooseConnector, applyConnector, cancelConnector } from "./fixtures/notification-editing";
import { afterEach, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { NotificationsPage } from "../src/context/factory/infrastructure/presentations/notifications";
import { newTelegramConnector } from "../src/context/factory/domain/notification-connectors";
import { noticeSettings, deferred } from "./fixtures/notices";
import type { Settings } from "../src/context/factory/domain/types";
afterEach(cleanup);
const withWork = (): Settings => ({...noticeSettings(),token_profiles:[{id:"work",label:"Work",account:"alice",has_token:true,owners:[],authors:[],reviewers:[]}]});

it("adds a disabled Telegram connector and discards the token draft", async () => {
  const user = userEvent.setup(); const onSave = vi.fn();
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  expect(screen.getByText("No notification connectors")).toBeTruthy();
  chooseConnector("Telegram");
  expect((screen.getByRole("combobox", {name:"Connector type"}) as HTMLSelectElement).value).toBe("telegram");
  expect((screen.getByRole("checkbox", {name:"Enabled"}) as HTMLInputElement).checked).toBe(false);
  const token = screen.getByLabelText("Bot token"); expect(token.getAttribute("type")).toBe("password");
  await user.type(token,"123456:fixture_only");
  applyConnector();
  await user.click(screen.getByRole("button", {name:"Discard changes"}));
  expect(screen.queryByLabelText("Bot token")).toBeNull();
  expect(screen.queryByRole("button", {name:"Edit connector Telegram"})).toBeNull();
  expect(onSave).not.toHaveBeenCalled();
});
it("saves active fields and clears secrets even when the save API returns void", async () => {
  const onSave = vi.fn(async (_s: Settings) => {});
  render(<NotificationsPage settings={withWork()} onSave={onSave} />);
  chooseConnector("Telegram");
  fireEvent.click(screen.getByRole("checkbox", {name:"Enabled"}));
  applyConnector(); fireEvent.click(screen.getByRole("button", {name:"Save changes"}));
  expect(within(screen.getByRole("dialog")).getByRole("alert").textContent).toContain("Chat ID"); expect(onSave).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Chat ID"), {target:{value:" -1001 "}});
  fireEvent.change(screen.getByLabelText("Bot token"), {target:{value:"123456:fixture_only"}});
  fireEvent.click(screen.getByRole("checkbox", {name:"Run started"}));
  fireEvent.click(screen.getByRole("checkbox", {name:"Work (@alice)"}));
  applyConnector(); fireEvent.click(screen.getByRole("button", {name:"Save changes"}));
  await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.chat_id).toBe("-1001");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].events).toEqual(["run_succeeded","run_failed","run_blocked"]);
  openConnector("Telegram");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("");
  expect(screen.getByText("Token configured")).toBeTruthy(); cancelConnector();
  expect((screen.getByRole("button",{name:"Save changes"}) as HTMLButtonElement).disabled).toBe(true);
});
it("isolates event edits and removes only the chosen connector before save", async () => {
  const settings = {...noticeSettings(), notification_connectors:[newTelegramConnector("a"), {...newTelegramConnector("b"),label:"Second"}]};
  const onSave = vi.fn(async (s: Settings) => s);
  render(<NotificationsPage settings={settings} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.click(screen.getByRole("checkbox",{name:"Run failed"})); applyConnector();
  openConnector("Second");
  expect((screen.getByRole("checkbox",{name:"Run failed"}) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("button",{name:"Remove connector Second"})); expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.map(c=>c.id)).toEqual(["a"]);
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].events).toEqual(["run_started","run_succeeded","run_blocked"]);
});
it("keeps dirty edits while props refresh and preserves other settings in save", async () => {
  const original = {...noticeSettings(),notification_connectors:[newTelegramConnector("a")]};
  const onSave = vi.fn(async (s: Settings) => s);
  const view = render(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Edited"}}); applyConnector();
  view.rerender(<NotificationsPage settings={{...original,max_concurrent:9}} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].max_concurrent).toBe(9); expect(onSave.mock.calls[0][0].notification_connectors?.[0].label).toBe("Edited");
});
it("disables controls during save, prevents duplicate requests, and preserves draft on safe failure", async () => {
  const pending = deferred<Settings>(); const onSave = vi.fn((_s: Settings)=>pending.promise);
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  chooseConnector("Telegram"); fireEvent.change(screen.getByLabelText("Bot token"),{target:{value:"PRIVATE_DRAFT"}}); applyConnector();
  const save=screen.getByRole("button",{name:"Save changes"}); fireEvent.click(save); fireEvent.click(save);
  expect(onSave).toHaveBeenCalledTimes(1);
  for (const name of ["Add connector", "Edit connector Telegram", "Discard changes"]) expect(screen.getByRole("button",{name}).matches(":disabled")).toBe(true);
  expect(screen.queryByRole("dialog")).toBeNull();
  pending.reject(new Error("request PRIVATE_DRAFT https://private.test"));
  await screen.findByRole("alert"); expect(screen.getByRole("alert").textContent).not.toContain("PRIVATE_DRAFT");
  openConnector("Telegram"); expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("PRIVATE_DRAFT"); cancelConnector();
  onSave.mockImplementation(async s=>s);
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave).toHaveBeenCalledTimes(2);
});
it("adopts clean external changes, discards dirty edits to latest props and preserves unsupported providers", async () => {
  const original = noticeSettings(); const onSave=vi.fn(async (s:Settings)=>s);
  const view=render(<NotificationsPage settings={original} onSave={onSave} />);
  const future={id:"future",label:"Future",enabled:true,events:[],type:"future",config:{x:7}};
  const incoming={...original,notification_connectors:[newTelegramConnector("external"),future]};
  view.rerender(<NotificationsPage settings={incoming} onSave={onSave} />);
  const list=screen.getByRole("list",{name:"Notification connectors"});
  expect(within(list).getAllByRole("button")).toHaveLength(2); expect(list.querySelectorAll("svg, img")).toHaveLength(2);
  openConnector("Future"); expect(screen.getByText("Unsupported connector type: future")).toBeTruthy();
  expect(screen.queryByRole("button",{name:"Apply changes"})).toBeNull(); cancelConnector();
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Local"}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Discard changes"}));
  openConnector("Telegram"); expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Telegram"); cancelConnector();
  expect(onSave).not.toHaveBeenCalled();
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Save"}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[1]).toEqual(future);
});
it("supports keyboard toggles and restores focus after removal", async () => {
  const user=userEvent.setup();
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[newTelegramConnector("a")]}} onSave={vi.fn()} />);
  openConnector("Telegram"); const enabled=screen.getByRole("checkbox",{name:"Enabled"}); enabled.focus(); await user.keyboard(" ");
  expect((enabled as HTMLInputElement).checked).toBe(true);
  await user.click(screen.getByRole("button",{name:"Remove connector Telegram"}));
  await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
});
it("ignores late saves after unmount and releases page busy state", async () => {
  const pending=deferred<Settings>(); const onDirty=vi.fn(); const onBusy=vi.fn();
  const view=render(<NotificationsPage settings={noticeSettings()} onSave={()=>pending.promise} onDirtyChange={onDirty} onBusyChange={onBusy} />);
  chooseConnector("Telegram"); applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
  expect(onBusy).toHaveBeenLastCalledWith(true); view.unmount();
  expect(onBusy).toHaveBeenLastCalledWith(false); expect(onDirty).toHaveBeenLastCalledWith(false);
  const calls=onBusy.mock.calls.length, dirtyCalls=onDirty.mock.calls.length;
  pending.resolve(noticeSettings()); await pending.promise;
  expect(onBusy).toHaveBeenCalledTimes(calls); expect(onDirty).toHaveBeenCalledTimes(dirtyCalls);
});
it("uses public save responses and never restores a submitted secret from old props", async () => {
  const original={...noticeSettings(),notification_connectors:[newTelegramConnector("a")]};
  const onSave=vi.fn(async (s: Settings)=>({...s,notification_connectors:s.notification_connectors?.map(c=>({...c,config:{...c.config,chat_id:"saved",has_token:true}}))}));
  const view=render(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Bot token"),{target:{value:"PRIVATE_SUBMITTED"}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  view.rerender(<NotificationsPage settings={{...original}} onSave={onSave} />);
  openConnector("Telegram"); cancelConnector(); openConnector("Telegram");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("");
  expect((screen.getByLabelText("Chat ID") as HTMLInputElement).value).toBe("saved");
});
it("keeps existing tokens with blank replacement and permits disabled incomplete drafts", async () => {
  const c=newTelegramConnector("a"); c.enabled=true; c.config={...c.config,chat_id:"@channel",has_token:true,token_profile_ids:["work"]};
  const onSave=vi.fn(async (s:Settings)=>s);
  render(<NotificationsPage settings={{...withWork(),notification_connectors:[c]}} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.bot_token).toBeUndefined();
  openConnector("Renamed"); fireEvent.click(screen.getByRole("checkbox",{name:"Enabled"})); fireEvent.change(screen.getByLabelText("Chat ID"),{target:{value:""}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
  expect(onSave.mock.calls[1][0].notification_connectors?.[0].enabled).toBe(false);
});
it("uses refreshed token metadata while retaining a dirty connector draft", async () => {
  const original={...withWork(),notification_connectors:[{...newTelegramConnector("a"),config:{...newTelegramConnector("a").config,token_profile_ids:["work"]}}]};
  const onSave=vi.fn(async (s:Settings)=>s);
  const view=render(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.click(screen.getByRole("checkbox",{name:"Enabled"})); fireEvent.change(screen.getByLabelText("Chat ID"),{target:{value:"@local_destination"}}); applyConnector();
  const incoming={...original,notification_connectors:[{...newTelegramConnector("a"),config:{...newTelegramConnector("a").config,chat_id:"@remote_destination",has_token:true,token_profile_ids:["work"]}}]};
  view.rerender(<NotificationsPage settings={incoming} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(1));
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config).toEqual({chat_id:"@local_destination",has_token:true,triggers:["new_issue","pr_approved","pr_review"],token_profile_ids:["work"]});
});
it("keeps connector and global action groups scoped to their own settings", () => {
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[newTelegramConnector("a"),{...newTelegramConnector("b"),label:"Second"}]}} onSave={vi.fn()} />);
  const page=screen.getByRole("region",{name:"Notifications"});
  const actions=within(page).getByRole("group",{name:"Notification settings actions"});
  expect(within(actions).getAllByRole("button").map(button=>button.textContent)).toEqual(["Save changes","Discard changes"]);
  for(const label of ["Telegram","Second"]) {
    const dialog=openConnector(label), local=within(dialog), group=local.getByRole("group",{name:"Connector actions"});
    expect(local.getByRole("button",{name:`Remove connector ${label}`})).toBeTruthy();
    expect(within(group).getAllByRole("button")).toHaveLength(3);
    expect(local.queryByRole("button",{name:"Save changes"})).toBeNull();
    const events=local.getByRole("group",{name:"Run events"});
    expect(within(events).getAllByRole("checkbox").map(c=>c.getAttribute("type"))).toEqual(["checkbox","checkbox","checkbox","checkbox"]);
    expect(Boolean(events.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true);
    expect(page.contains(dialog)).toBe(false); cancelConnector();
  }
});
it("keeps empty settings actions in context and can discard or save the final removal", async () => {
  const original={...noticeSettings(),notification_connectors:[newTelegramConnector("a")]};
  const onSave=vi.fn(async (s:Settings)=>s); const view=render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  expect(screen.getByText("No notification connectors")).toBeTruthy();
  const actions=screen.getByRole("group",{name:"Notification settings actions"});
  expect(within(actions).getAllByRole("button").every(b=>b.matches(":disabled"))).toBe(true);
  view.rerender(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.click(screen.getByRole("button",{name:"Remove connector Telegram"}));
  await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
  expect(within(actions).getAllByRole("button").every(b=>!b.matches(":disabled"))).toBe(true);
  fireEvent.click(within(actions).getByRole("button",{name:"Discard changes"}));
  expect(screen.getByRole("button",{name:"Edit connector Telegram"})).toBeTruthy(); expect(onSave).not.toHaveBeenCalled();
  openConnector("Telegram"); fireEvent.click(screen.getByRole("button",{name:"Remove connector Telegram"}));
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave).toHaveBeenCalledTimes(1); expect(onSave.mock.calls[0][0].notification_connectors).toEqual([]);
  expect(screen.getByText("No notification connectors")).toBeTruthy();
  expect(within(actions).getAllByRole("button").every(b=>b.matches(":disabled"))).toBe(true);
});
it("preserves keyboard order across connector fields and local actions", async () => {
  const user=userEvent.setup();
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[newTelegramConnector("a")]}} onSave={vi.fn()} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Edited"}});
  const targets=[screen.getByLabelText("Name"),screen.getByRole("checkbox",{name:"Enabled"}),screen.getByLabelText("Chat ID"),screen.getByLabelText("Bot token"),
    ...["Run started","Run succeeded","Run failed","Run blocked","New task","PR approved","Review requested"].map(name=>screen.getByRole("checkbox",{name})),
    screen.getByRole("button",{name:"Remove connector Edited"}),screen.getByRole("button",{name:"Cancel"}),screen.getByRole("button",{name:"Apply changes"}),screen.getByRole("button",{name:"Close connector editor"})];
  expect(screen.getByRole("combobox",{name:"Connector type"}).matches(":disabled")).toBe(true);
  for(const target of targets) { await user.tab(); expect(document.activeElement).toBe(target); }
  applyConnector();
  await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Edit connector Edited"})));
  await user.tab(); expect(document.activeElement).toBe(screen.getByRole("button",{name:"Save changes"}));
  await user.tab(); expect(document.activeElement).toBe(screen.getByRole("button",{name:"Discard changes"}));
});
it("keeps every page action disabled while saving", async () => {
  const pending=deferred<Settings>(); const onSave=vi.fn(()=>pending.promise);
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[newTelegramConnector("a")]}} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Edited"}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
  for(const name of ["Edit connector Edited","Add connector","Saving…","Discard changes"]) expect(screen.getByRole("button",{name}).matches(":disabled")).toBe(true);
  expect(onSave).toHaveBeenCalledTimes(1);
  const user=userEvent.setup(); await user.click(screen.getByRole("button",{name:"Add connector"}));
  await user.click(screen.getByRole("button",{name:"Edit connector Edited"})); expect(screen.queryByRole("dialog")).toBeNull();
  pending.resolve({...noticeSettings(),notification_connectors:[{...newTelegramConnector("a"),label:"Edited"}]});
  await screen.findByText("Notification settings saved.");
  expect(screen.getByRole("button",{name:"Add connector"}).matches(":disabled")).toBe(false);
});
it("keeps field and event errors associated with their controls", () => {
  const c=newTelegramConnector("a"); c.enabled=true; c.events=[];
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[c]}} onSave={vi.fn()} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:""}}); applyConnector();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
  for(const [label,hint] of [["Name","Name is required."],["Chat ID","Chat ID is required."],["Bot token","Bot token is required."]]) {
    const control=screen.getByLabelText(label); expect(control.getAttribute("aria-invalid")).toBe("true");
    expect(document.getElementById(control.getAttribute("aria-describedby")!)?.textContent).toBe(hint);
  }
  const events=screen.getByRole("group",{name:"Run events"}); expect(events.getAttribute("aria-invalid")).toBe("true");
  expect(document.getElementById(events.getAttribute("aria-describedby")!)?.textContent).toBe("Select at least one run event.");
  expect(within(screen.getByRole("dialog")).getByRole("alert").textContent).toContain("Select at least one run event.");
  expect(document.activeElement).toBe(screen.getByLabelText("Name"));
});
it.each([["Telegram","telegram"],["Google Chat","google_chat"]] as const)("adds a %s draft with defaults, keeps editor focus and saves only explicitly", async (label,kind) => {
  const user=userEvent.setup(), dirty=vi.fn(), onSave=vi.fn(async(s:Settings)=>s);
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} onDirtyChange={dirty} />);
  await user.click(screen.getByRole("button",{name:"Add connector"})); expect(dirty).not.toHaveBeenCalledWith(true); expect(onSave).not.toHaveBeenCalled();
  await user.click(within(screen.getByRole("dialog")).getByRole("button",{name:label}));
  const close=screen.getByRole("button",{name:"Close connector editor"});
  expect(screen.getAllByRole("dialog")).toHaveLength(1); expect(document.activeElement).toBe(close);
  await Promise.resolve(); expect(document.activeElement).toBe(close);
  expect((screen.getByRole("checkbox",{name:"Enabled"}) as HTMLInputElement).checked).toBe(false);
  expect(dirty).not.toHaveBeenCalledWith(true); applyConnector(); expect(dirty).toHaveBeenLastCalledWith(true); expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  const saved=onSave.mock.calls[0][0].notification_connectors![0]; expect(saved.id).toBeTruthy();
  expect(saved).toEqual({id:saved.id,label,enabled:false,events:["run_started","run_succeeded","run_failed","run_blocked"],type:kind,
    config:kind === "telegram" ? {chat_id:"",has_token:false,triggers:["new_issue","pr_approved","pr_review"],token_profile_ids:[]} : {has_webhook_url:false,triggers:["new_issue","pr_approved","pr_review"],token_profile_ids:[]}});
  expect(onSave).toHaveBeenCalledTimes(1);
});
it.each(["escape","close"])("preserves dirty fields when cancelling selector via %s", async method => {
  const user=userEvent.setup(), onSave=vi.fn(), dirty=vi.fn();
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[newTelegramConnector("existing")]}} onSave={onSave} onDirtyChange={dirty} />);
  openConnector("Telegram"); await user.clear(screen.getByLabelText("Name")); await user.type(screen.getByLabelText("Name"),"Edited draft");
  await user.type(screen.getByLabelText("Bot token"),"fixture_draft"); applyConnector();
  const dirtyCalls=dirty.mock.calls.length; await user.click(screen.getByRole("button",{name:"Add connector"}));
  if(method === "escape") await user.keyboard("{Escape}"); else await user.click(screen.getByRole("button",{name:"Close connector selector"}));
  expect(screen.queryByRole("dialog")).toBeNull(); expect(dirty.mock.calls.length).toBe(dirtyCalls);
  expect(dirty).toHaveBeenLastCalledWith(true); expect(onSave).not.toHaveBeenCalled();
  openConnector("Edited draft"); expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Edited draft");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("fixture_draft"); cancelConnector();
  fireEvent.click(screen.getByRole("button",{name:"Discard changes"})); openConnector("Telegram");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("");
});
it.each(["Telegram","Google Chat"] as const)("can add repeated %s instances without changing existing connectors", async label => {
  const existing={...newTelegramConnector("existing"),label:"Existing untouched"}, onSave=vi.fn(async(s:Settings)=>s);
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[existing]}} onSave={onSave} />);
  for(let index=0;index<2;index+=1) { chooseConnector(label); applyConnector(); }
  expect(screen.getAllByRole("button",{name:`Edit connector ${label}`})).toHaveLength(2); expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
  const connectors=onSave.mock.calls[0][0].notification_connectors!;
  expect(connectors).toHaveLength(3); expect(new Set(connectors.map(c=>c.id)).size).toBe(3); expect(connectors[0]).toEqual(existing);
  expect(connectors.slice(1).map(c=>c.type)).toEqual([label === "Telegram" ? "telegram" : "google_chat",label === "Telegram" ? "telegram" : "google_chat"]);
});

it("keeps modal edits isolated until apply and cancel preserves the page draft", () => {
  const onSave = vi.fn(async (s: Settings) => s);
  const dirty = vi.fn();
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [
    newTelegramConnector("a"), {...newTelegramConnector("b"), label: "Second"},
  ]}} onSave={onSave} onDirtyChange={dirty} />);
  expect(screen.queryByLabelText("Bot token")).toBeNull();
  expect(screen.queryByLabelText("Name")).toBeNull();
  openConnector("Telegram");
  fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Applied"}});
  fireEvent.change(screen.getByLabelText("Bot token"), {target: {value: "fixture_secret"}});
  expect(dirty).not.toHaveBeenCalledWith(true);
  applyConnector();
  expect(screen.getByRole("button", {name: "Edit connector Applied"})).toBeTruthy();
  expect(dirty).toHaveBeenLastCalledWith(true);
  openConnector("Applied");
  fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Cancelled"}});
  fireEvent.change(screen.getByLabelText("Bot token"), {target: {value: "cancelled_secret"}});
  cancelConnector();
  expect(screen.queryByRole("button", {name: "Edit connector Cancelled"})).toBeNull();
  openConnector("Applied");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("fixture_secret");
  cancelConnector();
  expect(screen.getByRole("button", {name: "Edit connector Second"})).toBeTruthy();
  expect(onSave).not.toHaveBeenCalled();
});

it.each(["Telegram", "Google Chat"] as const)("stages %s creation only after apply", label => {
  const onSave = vi.fn();
  const dirty = vi.fn();
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} onDirtyChange={dirty} />);
  chooseConnector(label);
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  expect((screen.getByRole("checkbox", {name: "Enabled"}) as HTMLInputElement).checked).toBe(false);
  expect(dirty).not.toHaveBeenCalledWith(true);
  cancelConnector();
  expect(screen.queryByRole("button", {name: `Edit connector ${label}`})).toBeNull();
  expect(dirty).not.toHaveBeenCalledWith(true);
  chooseConnector(label);
  applyConnector();
  expect(screen.getByRole("button", {name: `Edit connector ${label}`})).toBeTruthy();
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect(onSave).not.toHaveBeenCalled();
});
