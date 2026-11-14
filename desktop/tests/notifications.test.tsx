import { afterEach, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { NotificationsPage } from "../src/context/factory/infrastructure/presentations/notifications";
import { newTelegramConnector } from "../src/context/factory/domain/notification-connectors";
import { noticeSettings, deferred } from "./fixtures/notices";
import type { Settings } from "../src/context/factory/domain/types";
afterEach(cleanup);
it("adds a disabled Telegram connector and discards the token draft", async () => {
  const user = userEvent.setup(); const onSave = vi.fn();
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  expect(screen.getByText("No notification connectors")).toBeTruthy();
  await user.click(screen.getByRole("button", {name:"Add connector"}));
  await user.click(within(screen.getByRole("dialog", {name:"Choose a connector"})).getByRole("button", {name:"Telegram"}));
  expect((screen.getByRole("combobox", {name:"Connector type"}) as HTMLSelectElement).value).toBe("telegram");
  expect((screen.getByRole("checkbox", {name:"Enabled"}) as HTMLInputElement).checked).toBe(false);
  const token = screen.getByLabelText("Bot token"); expect(token.getAttribute("type")).toBe("password");
  await user.type(token,"123456:fixture_only");
  await user.click(screen.getByRole("button", {name:"Discard changes"}));
  expect(screen.queryByLabelText("Bot token")).toBeNull();expect(onSave).not.toHaveBeenCalled();
});
it("saves active fields and clears secrets even when the save API returns void", async () => {
  const onSave = vi.fn(async (_s: Settings) => {});
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button", {name:"Add connector"}));
  fireEvent.click(within(screen.getByRole("dialog", {name:"Choose a connector"})).getByRole("button", {name:"Telegram"}));
  fireEvent.click(screen.getByRole("checkbox", {name:"Enabled"}));
  fireEvent.click(screen.getByRole("button", {name:"Save changes"}));
  expect(screen.getByRole("alert").textContent).toContain("Chat ID");expect(onSave).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText("Chat ID"), {target:{value:" -1001 "}});
  fireEvent.change(screen.getByLabelText("Bot token"), {target:{value:"123456:fixture_only"}});
  fireEvent.click(screen.getByRole("checkbox", {name:"Run started"}));
  fireEvent.click(screen.getByRole("button", {name:"Save changes"}));
  await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.chat_id).toBe("-1001");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].events).toEqual(["run_succeeded","run_failed","run_blocked"]);
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("");
  expect(screen.getByText("Token configured")).toBeTruthy();
  expect((screen.getByRole("button",{name:"Save changes"}) as HTMLButtonElement).disabled).toBe(true);
});
it("isolates event edits and removes only the chosen connector before save", async () => {
  const settings = {...noticeSettings(), notification_connectors:[newTelegramConnector("a"), {...newTelegramConnector("b"),label:"Second"}]};
  const onSave = vi.fn(async (s: Settings) => s);
  render(<NotificationsPage settings={settings} onSave={onSave} />);
  fireEvent.click(screen.getAllByRole("checkbox",{name:"Run failed"})[0]);
  expect((screen.getAllByRole("checkbox",{name:"Run failed"})[1] as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("button",{name:"Remove connector Second"}));expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.map(c=>c.id)).toEqual(["a"]);
});
it("keeps dirty edits while props refresh and preserves other settings in save", async () => {
  const original = {...noticeSettings(),notification_connectors:[newTelegramConnector("a")]};
  const onSave = vi.fn(async (s: Settings) => s);
  const view = render(<NotificationsPage settings={original} onSave={onSave} />);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Edited"}});
  view.rerender(<NotificationsPage settings={{...original,max_concurrent:9}} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].max_concurrent).toBe(9);expect(onSave.mock.calls[0][0].notification_connectors?.[0].label).toBe("Edited");
});
it("disables controls during save, prevents duplicate requests, and preserves draft on safe failure", async () => {
  const pending = deferred<Settings>();const onSave = vi.fn(()=>pending.promise);
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button",{name:"Add connector"}));
  fireEvent.click(within(screen.getByRole("dialog", {name:"Choose a connector"})).getByRole("button", {name:"Telegram"}));
  fireEvent.change(screen.getByLabelText("Bot token"),{target:{value:"PRIVATE_DRAFT"}});
  const save=screen.getByRole("button",{name:"Save changes"});fireEvent.click(save);fireEvent.click(save);
  expect(onSave).toHaveBeenCalledTimes(1);expect((screen.getByLabelText("Bot token") as HTMLInputElement).closest("fieldset")?.disabled).toBe(true);
  pending.reject(new Error("request PRIVATE_DRAFT https://private.test"));
  await screen.findByRole("alert");expect(screen.getByRole("alert").textContent).not.toContain("PRIVATE_DRAFT");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("PRIVATE_DRAFT");
});
it("adopts clean external changes, discards dirty edits to latest props and preserves unsupported providers", async () => {
  const original = noticeSettings();const onSave=vi.fn(async (s:Settings)=>s);
  const view=render(<NotificationsPage settings={original} onSave={onSave} />);
  const incoming={...original,notification_connectors:[newTelegramConnector("external"),{id:"future",label:"Future",enabled:true,events:[],type:"future",config:{x:7}}]};
  view.rerender(<NotificationsPage settings={incoming} onSave={onSave} />);
  expect(screen.getByText("Unsupported connector type: future")).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Local"}});
  fireEvent.click(screen.getByRole("button",{name:"Discard changes"}));
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Telegram");
  expect(onSave).not.toHaveBeenCalled();
});
it("supports keyboard toggles and restores focus after removal", async () => {
  const user=userEvent.setup();
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[newTelegramConnector("a")]}} onSave={vi.fn()} />);
  const enabled=screen.getByRole("checkbox",{name:"Enabled"});enabled.focus();await user.keyboard(" ");
  expect((enabled as HTMLInputElement).checked).toBe(true);
  await user.click(screen.getByRole("button",{name:"Remove connector Telegram"}));
  await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
});
it("ignores late saves after unmount and releases page busy state", async () => {
  const pending=deferred<Settings>();const onDirty=vi.fn();const onBusy=vi.fn();
  const view=render(<NotificationsPage settings={noticeSettings()} onSave={()=>pending.promise} onDirtyChange={onDirty} onBusyChange={onBusy} />);
  fireEvent.click(screen.getByRole("button",{name:"Add connector"}));
  fireEvent.click(within(screen.getByRole("dialog", {name:"Choose a connector"})).getByRole("button", {name:"Telegram"}));fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
  expect(onBusy).toHaveBeenLastCalledWith(true);view.unmount();
  expect(onBusy).toHaveBeenLastCalledWith(false);expect(onDirty).toHaveBeenLastCalledWith(false);
  const calls=onBusy.mock.calls.length;
  pending.resolve(noticeSettings());await pending.promise;
  expect(onBusy).toHaveBeenCalledTimes(calls);
});
it("uses public save responses and never restores a submitted secret from old props", async () => {
  const original={...noticeSettings(),notification_connectors:[newTelegramConnector("a")]};
  const onSave=vi.fn(async (s: Settings)=>({...s,notification_connectors:s.notification_connectors?.map(c=>({...c,config:{chat_id:"saved",has_token:true}}))}));
  const view=render(<NotificationsPage settings={original} onSave={onSave} />);
  fireEvent.change(screen.getByLabelText("Bot token"),{target:{value:"PRIVATE_SUBMITTED"}});
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
  view.rerender(<NotificationsPage settings={{...original}} onSave={onSave} />);
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("");
  expect((screen.getByLabelText("Chat ID") as HTMLInputElement).value).toBe("saved");
});
it("keeps existing tokens with blank replacement and permits disabled incomplete drafts", async () => {
  const c=newTelegramConnector("a");c.enabled=true;c.config={chat_id:"@channel",has_token:true};
  const onSave=vi.fn(async (s:Settings)=>s);
  render(<NotificationsPage settings={{...noticeSettings(),notification_connectors:[c]}} onSave={onSave} />);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}});fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config.bot_token).toBeUndefined();
  fireEvent.click(screen.getByRole("checkbox",{name:"Enabled"}));fireEvent.change(screen.getByLabelText("Chat ID"),{target:{value:""}});
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
  expect(onSave.mock.calls[1][0].notification_connectors?.[0].enabled).toBe(false);
});
it("uses refreshed token metadata while retaining a dirty connector draft", async () => {
  const original={...noticeSettings(),notification_connectors:[newTelegramConnector("a")]};
  const onSave=vi.fn(async (s:Settings)=>s);
  const view=render(<NotificationsPage settings={original} onSave={onSave} />);
  fireEvent.click(screen.getByRole("checkbox",{name:"Enabled"}));
  fireEvent.change(screen.getByLabelText("Chat ID"),{target:{value:"@local_destination"}});
  const incoming={...original,notification_connectors:[{...newTelegramConnector("a"),config:{chat_id:"@remote_destination",has_token:true}}]};
  view.rerender(<NotificationsPage settings={incoming} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
  await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(1));
  expect(onSave.mock.calls[0][0].notification_connectors?.[0].config).toEqual({chat_id:"@local_destination",has_token:true});
});

it("keeps connector and global action groups scoped to their own settings", () => {
  const settings = {
    ...noticeSettings(),
    notification_connectors: [newTelegramConnector("a"), {...newTelegramConnector("b"), label: "Second"}],
  };
  render(<NotificationsPage settings={settings} onSave={vi.fn()} />);
  const page = within(screen.getByRole("region", {name: "Notifications"}));
  const first = page.getByRole("group", {name: "Telegram"});
  const second = page.getByRole("group", {name: "Second"});
  for (const [card, label] of [[first, "Telegram"], [second, "Second"]] as const) {
    const local = within(card);
    const actions = local.getByRole("group", {name: "Connector actions"});
    const remove = within(actions).getByRole("button", {name: `Remove connector ${label}`});
    expect(within(actions).getAllByRole("button")).toEqual([remove]);
    expect(local.queryByRole("button", {name: "Save changes"})).toBeNull();
    const events = local.getByRole("group", {name: "Run events"});
    expect(within(events).getAllByRole("checkbox").map(input => input.getAttribute("type")))
      .toEqual(["checkbox", "checkbox", "checkbox", "checkbox"]);
    expect(Boolean(events.compareDocumentPosition(actions) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true);
  }
  const actions = page.getByRole("group", {name: "Notification settings actions"});
  expect(within(actions).getAllByRole("button").map(button => button.textContent))
    .toEqual(["Save changes", "Discard changes"]);
  expect(Boolean(second.compareDocumentPosition(actions) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true);
});

it("keeps empty settings actions in context and can discard or save the final removal", async () => {
  const original = {...noticeSettings(), notification_connectors: [newTelegramConnector("a")]};
  const onSave = vi.fn(async (settings: Settings) => settings);
  const view = render(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  const page = screen.getByRole("region", {name: "Notifications"});
  expect(within(page).getByText("No notification connectors")).toBeTruthy();
  let actions = within(page).getByRole("group", {name: "Notification settings actions"});
  expect(within(actions).getAllByRole("button").every(button => button.matches(":disabled"))).toBe(true);
  expect(within(page).queryByRole("group", {name: "Telegram"})).toBeNull();
  view.rerender(<NotificationsPage settings={original} onSave={onSave} />);
  fireEvent.click(screen.getByRole("button", {name: "Remove connector Telegram"}));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Add connector"})));
  actions = within(page).getByRole("group", {name: "Notification settings actions"});
  expect(within(actions).getAllByRole("button").every(button => !button.matches(":disabled"))).toBe(true);
  fireEvent.click(within(actions).getByRole("button", {name: "Discard changes"}));
  expect(screen.getByRole("group", {name: "Telegram"})).toBeTruthy();
  expect(onSave).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", {name: "Remove connector Telegram"}));
  fireEvent.click(screen.getByRole("button", {name: "Save changes"}));
  await screen.findByText("Notification settings saved.");
  expect(onSave).toHaveBeenCalledTimes(1);
  expect(onSave.mock.calls[0][0].notification_connectors).toEqual([]);
  expect(screen.getByText("No notification connectors")).toBeTruthy();
  expect(within(actions).getAllByRole("button").every(button => button.matches(":disabled"))).toBe(true);
});

it("preserves keyboard order across connector fields and both action levels", async () => {
  const user = userEvent.setup();
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [newTelegramConnector("a")]}} onSave={vi.fn()} />);
  fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Edited"}});
  screen.getByRole("button", {name: "Add connector"}).focus();
  const targets = [
    screen.getByLabelText("Name"),
    screen.getByRole("checkbox", {name: "Enabled"}),
    screen.getByLabelText("Chat ID"),
    screen.getByLabelText("Bot token"),
    screen.getByRole("checkbox", {name: "Run started"}),
    screen.getByRole("checkbox", {name: "Run succeeded"}),
    screen.getByRole("checkbox", {name: "Run failed"}),
    screen.getByRole("checkbox", {name: "Run blocked"}),
    screen.getByRole("button", {name: "Remove connector Edited"}),
    screen.getByRole("button", {name: "Save changes"}),
    screen.getByRole("button", {name: "Discard changes"}),
  ];
  expect(screen.getByRole("combobox", {name: "Connector type"}).matches(":disabled")).toBe(true);
  for (const target of targets) {
    await user.tab();
    expect(document.activeElement).toBe(target);
  }
});

it("keeps every action group and editor disabled while saving", async () => {
  const pending = deferred<Settings>();
  const onSave = vi.fn(() => pending.promise);
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [newTelegramConnector("a")]}} onSave={onSave} />);
  fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Edited"}});
  fireEvent.click(screen.getByRole("button", {name: "Save changes"}));
  const card = screen.getByRole("group", {name: "Edited"});
  expect((card as HTMLFieldSetElement).disabled).toBe(true);
  for (const control of card.querySelectorAll("input, select, button")) {
    expect(control.matches(":disabled")).toBe(true);
  }
  expect(screen.getByRole("button", {name: "Add connector"}).matches(":disabled")).toBe(true);
  const actions = screen.getByRole("group", {name: "Notification settings actions"});
  expect(within(actions).getByRole("button", {name: "Saving…"}).matches(":disabled")).toBe(true);
  expect(within(actions).getByRole("button", {name: "Discard changes"}).matches(":disabled")).toBe(true);
  expect(onSave).toHaveBeenCalledTimes(1);
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", {name: "Add connector"}));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(onSave).toHaveBeenCalledTimes(1);
  pending.resolve({...noticeSettings(), notification_connectors: [{...newTelegramConnector("a"), label: "Edited"}]});
  await screen.findByText("Notification settings saved.");
  expect(screen.getByRole("button", {name: "Add connector"}).matches(":disabled")).toBe(false);
});

it("keeps field and event errors associated with their controls", () => {
  const connector = newTelegramConnector("a");
  connector.enabled = true;
  connector.events = [];
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [connector]}} onSave={vi.fn()} />);
  fireEvent.change(screen.getByLabelText("Name"), {target: {value: ""}});
  fireEvent.click(screen.getByRole("button", {name: "Save changes"}));
  for (const [label, hint] of [["Name", "Name is required."], ["Chat ID", "Chat ID is required."], ["Bot token", "Bot token is required."]]) {
    const control = screen.getByLabelText(label);
    expect(control.getAttribute("aria-invalid")).toBe("true");
    const describedBy = control.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy!)?.textContent).toBe(hint);
  }
  const events = screen.getByRole("group", {name: "Run events"});
  expect(events.getAttribute("aria-invalid")).toBe("true");
  expect(document.getElementById(events.getAttribute("aria-describedby")!)?.textContent)
    .toBe("Select at least one run event.");
  expect(screen.getByRole("alert").textContent).toContain("Select at least one run event.");
});

it.each([
  ["Telegram", "telegram"],
  ["Google Chat", "google_chat"],
] as const)("adds a %s draft with defaults, keeps editor focus and saves only explicitly", async (label, kind) => {
  const user = userEvent.setup(); const dirty = vi.fn();
  const onSave = vi.fn(async (value: Settings) => value);
  render(<NotificationsPage settings={noticeSettings()} onSave={onSave} onDirtyChange={dirty} />);
  await user.click(screen.getByRole("button", { name: "Add connector" }));
  expect(dirty).not.toHaveBeenCalledWith(true);
  expect(onSave).not.toHaveBeenCalled();
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: label }));
  expect(screen.queryByRole("dialog")).toBeNull();
  const card = screen.getByRole("group", { name: label });
  await waitFor(() => expect(document.activeElement).toBe(card));
  // Volver a ceder a las microtasks detecta una restauración tardía incorrecta del opener.
  await Promise.resolve(); expect(document.activeElement).toBe(card);
  expect((within(card).getByRole("checkbox", { name: "Enabled" }) as HTMLInputElement).checked).toBe(false);
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect(onSave).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Save changes" }));
  await screen.findByText("Notification settings saved.");
  const saved = onSave.mock.calls[0][0].notification_connectors![0];
  expect(saved.id).toBeTruthy();
  expect(saved).toEqual({
    id: saved.id, label, enabled: false,
    events: ["run_started", "run_succeeded", "run_failed", "run_blocked"],
    type: kind,
    config: kind === "telegram"
      ? {chat_id: "", has_token: false}
      : {has_webhook_url: false, triggers: ["new_issue", "pr_approved", "pr_review"], token_profile_ids: []},
  });
  expect(onSave).toHaveBeenCalledTimes(1);
});

it.each(["escape", "close"])("preserves dirty fields when cancelling via %s", async method => {
  const user = userEvent.setup(); const onSave = vi.fn(); const dirty = vi.fn();
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [newTelegramConnector("existing")]}}
    onSave={onSave} onDirtyChange={dirty} />);
  await user.clear(screen.getByLabelText("Name")); await user.type(screen.getByLabelText("Name"), "Edited draft");
  await user.type(screen.getByLabelText("Bot token"), "fixture_draft");
  const dirtyCalls = dirty.mock.calls.length;
  await user.click(screen.getByRole("button", { name: "Add connector" }));
  if (method === "escape") await user.keyboard("{Escape}");
  else await user.click(screen.getByRole("button", { name: "Close connector selector" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getAllByLabelText("Name")).toHaveLength(1);
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Edited draft");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("fixture_draft");
  expect(dirty.mock.calls.length).toBe(dirtyCalls);
  expect(dirty).toHaveBeenLastCalledWith(true); expect(onSave).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Discard changes" }));
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Telegram");
  expect((screen.getByLabelText("Bot token") as HTMLInputElement).value).toBe("");
});

it.each(["Telegram", "Google Chat"])("can add repeated %s instances without changing existing connectors", async label => {
  const user = userEvent.setup();
  const existing = {...newTelegramConnector("existing"), label: "Existing untouched"};
  const onSave = vi.fn(async (value: Settings) => value);
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [existing]}} onSave={onSave} />);
  for (let index = 0; index < 2; index += 1) {
    await user.click(screen.getByRole("button", { name: "Add connector" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("button", { name: "Telegram" })).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Google Chat" })).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: label }));
  }
  expect(screen.getAllByRole("group", { name: label })).toHaveLength(2);
  expect(onSave).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Save changes" }));
  await screen.findByText("Notification settings saved.");
  const connectors = onSave.mock.calls[0][0].notification_connectors!;
  expect(connectors).toHaveLength(3);
  expect(new Set(connectors.map(item => item.id)).size).toBe(3);
  expect(connectors[0]).toEqual(existing);
  expect(connectors.slice(1).map(item => item.type)).toEqual([label === "Telegram" ? "telegram" : "google_chat", label === "Telegram" ? "telegram" : "google_chat"]);
});
