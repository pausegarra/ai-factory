import { openConnector, applyConnector, cancelConnector } from "./fixtures/notification-editing";
import { afterEach, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { NotificationsPage } from "../src/context/factory/infrastructure/presentations/notifications";
import { newTelegramConnector, newGoogleChatConnector, cleanNotificationConnectors } from "../src/context/factory/domain/notification-connectors";
import type { Settings } from "../src/context/factory/domain/types";
import { noticeSettings, deferred } from "./fixtures/notices";
afterEach(cleanup);
const profile = {id:"work",label:"Work",type: "github_issues" as const, account:"alice",has_token:true,owners:[],authors:[],reviewers:[]};
const checked = (element: HTMLElement) => (element as HTMLInputElement).checked;

it("renders identical filter choices for both providers", () => {
  render(<NotificationsPage settings={{...noticeSettings(),source_connectors:[profile],notification_connectors:[newTelegramConnector("tg"),newGoogleChatConnector("chat")]}} onSave={vi.fn()} />);
  for(const name of ["Telegram","Google Chat"]) {
    const card = within(openConnector(name));
    expect(within(card.getByRole("group",{name:"Run events"})).getAllByRole("checkbox").map(c=>c.closest("label")?.textContent)).toEqual(["Run started","Run succeeded","Run failed","Run blocked"]);
    expect(within(card.getByRole("group",{name:"GitHub triggers"})).getAllByRole("checkbox").map(c=>c.closest("label")?.textContent)).toEqual(["New task","PR approved","Review requested","Release"]);
    for(const label of ["New task","PR approved","Review requested","Release"]) expect(checked(card.getByRole("checkbox",{name:label}))).toBe(true);
    expect(checked(card.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(false); cancelConnector();
  }
});

it("isolates all three groups across two Telegram connectors and Google Chat", async () => {
  const connectors = [newTelegramConnector("a"),{...newTelegramConnector("b"),label:"Second"},newGoogleChatConnector("c")];
  const onSave = vi.fn(async (s:Settings)=>({...s,notification_connectors:cleanNotificationConnectors(s.notification_connectors ?? [])}));
  render(<NotificationsPage settings={{...noticeSettings(),source_connectors:[profile],notification_connectors:connectors}} onSave={onSave} />);
  const card = within(openConnector("Telegram"));
  for(const name of ["Run started","New task","Work (@alice)"]) fireEvent.click(card.getByRole("checkbox",{name}));
  applyConnector();
  for(const name of ["Second","Google Chat"]) {
    const other = within(openConnector(name));
    expect(checked(other.getByRole("checkbox",{name:"Run started"}))).toBe(true);
    expect(checked(other.getByRole("checkbox",{name:"New task"}))).toBe(true);
    expect(checked(other.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(false); cancelConnector();
  }
  fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
  await screen.findByText("Notification settings saved.");
  const saved = onSave.mock.calls[0][0].notification_connectors!;
  expect(saved[0].events).toEqual(["run_succeeded","run_failed","run_blocked"]);
  expect(saved[0].config).toEqual({chat_id:"",has_token:false,triggers:["pr_approved","pr_review","release"],token_profile_ids:["work"]});
  expect(saved.slice(1)).toEqual(connectors.slice(1));
});

for(const create of [newTelegramConnector,newGoogleChatConnector]) {
  const kind = create("x").type;
  function settings(enabled = false): Settings {
    const c = create("a"); c.enabled = enabled;
    if(c.type === "telegram") {c.config.chat_id = "fixture"; c.config.has_token = true;}
    else c.config.has_webhook_url = true;
    return {...noticeSettings(),source_connectors:[profile],notification_connectors:[c]};
  }
  it(`${kind}: persists filters through save, reopen and discard`, async () => {
    let stored = settings();
    const onSave = vi.fn(async (s:Settings)=>{stored={...s,notification_connectors:cleanNotificationConnectors(s.notification_connectors ?? [])};return stored;});
    const view = render(<NotificationsPage settings={stored} onSave={onSave} />);
    openConnector(create("a").label);
    for(const name of ["Run started","New task","Work (@alice)"]) fireEvent.click(screen.getByRole("checkbox",{name}));
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await screen.findByText("Notification settings saved.");
    const saved = structuredClone(stored.notification_connectors![0]);
    view.unmount();
    render(<NotificationsPage settings={stored} onSave={onSave} />);
    openConnector(create("a").label);
    expect(checked(screen.getByRole("checkbox",{name:"Run started"}))).toBe(false);
    expect(checked(screen.getByRole("checkbox",{name:"New task"}))).toBe(false);
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(true);
    const secretLabel = kind === "telegram" ? "Bot token" : "Webhook URL";
    expect(screen.getByLabelText(secretLabel).getAttribute("type")).toBe("password");
    fireEvent.change(screen.getByLabelText(secretLabel),{target:{value:"PRIVATE_DRAFT"}});
    for(const name of ["Run started","New task","Work (@alice)"]) fireEvent.click(screen.getByRole("checkbox",{name}));
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Discard changes"})); openConnector(create("a").label);
    expect(checked(screen.getByRole("checkbox",{name:"New task"}))).toBe(false);
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(true);
    expect((screen.getByLabelText(secretLabel) as HTMLInputElement).value).toBe("");
    fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}});
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"})); await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
    expect(stored.notification_connectors![0]).toEqual({...saved,label:"Renamed"});
  });
  it(`${kind}: associates required filter errors and allows empty disabled drafts`, async () => {
    const original = settings(true); const c = original.notification_connectors![0];
    c.events=[];c.config.triggers=[];c.config.token_profile_ids=[];
    const onSave = vi.fn(async (s:Settings)=>s);
    render(<NotificationsPage settings={original} onSave={onSave} />);
    openConnector(create("a").label);
    fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Changed"}});
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
    expect(onSave).not.toHaveBeenCalled();
    for(const name of ["Run events","GitHub triggers","GitHub sources"]) {
      const group = screen.getByRole("group",{name});
      expect(group.getAttribute("aria-invalid")).toBe("true");
      expect(document.getElementById(group.getAttribute("aria-describedby")!)?.textContent).toMatch(/Select/);
    }
    for(const name of ["Run failed","Review requested","Work (@alice)"]) fireEvent.click(screen.getByRole("checkbox",{name}));
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
    expect(onSave).toHaveBeenCalledTimes(1);
    openConnector("Changed");
    for(const name of ["Run failed","Review requested","Work (@alice)","Enabled"]) fireEvent.click(screen.getByRole("checkbox",{name}));
    fireEvent.change(screen.getByLabelText(kind === "telegram" ? "Chat ID" : "Webhook URL"),{target:{value:""}});
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
    expect(onSave.mock.calls[1][0].notification_connectors![0].config.token_profile_ids).toEqual([]);
  });
  it(`${kind}: retains stable IDs on profile rename and exposes deleted selections`, async () => {
    const original = settings(true);original.notification_connectors![0].config.token_profile_ids=["work"];
    const onSave=vi.fn(async (s:Settings)=>s);
    const view=render(<NotificationsPage settings={original} onSave={onSave} />);
    openConnector(create("a").label);
    fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Dirty"}});
    view.rerender(<NotificationsPage settings={{...original,source_connectors:[{...profile,label:"Renamed",token:"PRIVATE_GITHUB"},{...profile,id:"new",label:"New",account:"bob"}]}} onSave={onSave} />);
    expect(checked(screen.getByRole("checkbox",{name:"Renamed (@alice)"}))).toBe(true);
    expect(checked(screen.getByRole("checkbox",{name:"New (@bob)"}))).toBe(false);
    expect(document.body.innerHTML).not.toContain("PRIVATE_GITHUB");
    const incoming={...original,source_connectors:[{...profile,id:"new",label:"New",account:"bob"}]};
    view.rerender(<NotificationsPage settings={incoming} onSave={onSave} />);
    expect(checked(screen.getByRole("checkbox",{name:"Unavailable source (work)"}))).toBe(true);
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));expect(onSave).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("checkbox",{name:"Unavailable source (work)"}));
    fireEvent.click(screen.getByRole("checkbox",{name:"New (@bob)"}));
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
    expect(onSave.mock.calls[0][0].notification_connectors![0].config.token_profile_ids).toEqual(["new"]);
  });
  it(`${kind}: preserves filter drafts on pending failure and void success`, async () => {
    const pending=deferred<Settings | void>(); const onSave=vi.fn(()=>pending.promise);
    render(<NotificationsPage settings={settings()} onSave={onSave} />);
    openConnector(create("a").label);
    fireEvent.click(screen.getByRole("checkbox",{name:"New task"}));
    fireEvent.click(screen.getByRole("checkbox",{name:"Work (@alice)"}));
    applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));
    fireEvent.click(screen.getByRole("button",{name:"Saving…"}));
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button",{name:`Edit connector ${create("a").label}`}).matches(":disabled")).toBe(true); expect(screen.queryByRole("dialog")).toBeNull();
    pending.reject(new Error("PRIVATE_ERROR"));await screen.findByRole("alert");
    expect(screen.getByRole("alert").textContent).not.toContain("PRIVATE_ERROR");
    openConnector(create("a").label);
    expect(checked(screen.getByRole("checkbox",{name:"New task"}))).toBe(false);
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(true);
    cancelConnector();
    onSave.mockImplementation(async()=>undefined);
    fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
    openConnector(create("a").label);
    expect(checked(screen.getByRole("checkbox",{name:"New task"}))).toBe(false);
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(true);
  });
  it(`${kind}: reaches triggers and profiles with Tab and toggles only the focused choice`, async () => {
    const user=userEvent.setup();
    render(<NotificationsPage settings={settings()} onSave={vi.fn()} />);
    openConnector(create("a").label);
    screen.getByRole("checkbox",{name:"Run blocked"}).focus();
    for(const name of ["New task","PR approved","Review requested","Release","Work (@alice)"]) {
      await user.tab();expect(document.activeElement).toBe(screen.getByRole("checkbox",{name}));
    }
    await user.keyboard(" ");
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice)"}))).toBe(true);
    expect(checked(screen.getByRole("checkbox",{name:"Review requested"}))).toBe(true);
    await user.tab();expect(document.activeElement).toBe(screen.getByRole("button",{name:`Remove connector ${create("a").label}`}));
  });
  it(`${kind}: distinguishes duplicate profile labels by stable ID`, () => {
    const original=settings();original.source_connectors=[profile,{...profile,id:"other"}];
    render(<NotificationsPage settings={original} onSave={vi.fn()} />);
    openConnector(create("a").label);
    fireEvent.click(screen.getByRole("checkbox",{name:"Work (@alice) (other)"}));
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice) (work)"}))).toBe(false);
    expect(checked(screen.getByRole("checkbox",{name:"Work (@alice) (other)"}))).toBe(true);
  });
}

it("persists only the edited Release filter and leaves explicit older selections intact", async () => {
  const telegram = newTelegramConnector("tg");
  telegram.config.triggers = ["new_issue"];
  const chat = newGoogleChatConnector("chat");
  const original = {...noticeSettings(), source_connectors:[profile], notification_connectors:[telegram, chat]};
  const save = vi.fn(async (s: Settings) => s);
  const view = render(<NotificationsPage settings={original} onSave={save} />);
  openConnector("Telegram");
  expect(checked(screen.getByRole("checkbox", {name:"Release"}))).toBe(false);
  fireEvent.change(screen.getByLabelText("Name"), {target:{value:"Older selection"}});
  applyConnector();
  fireEvent.click(screen.getByRole("button", {name:"Save changes"}));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0][0].notification_connectors![0].config.triggers).toEqual(["new_issue"]);
  openConnector("Older selection");
  fireEvent.click(screen.getByRole("checkbox", {name:"Release"}));
  applyConnector();
  fireEvent.click(screen.getByRole("button", {name:"Save changes"}));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  const stored = save.mock.calls[1][0];
  expect(stored.notification_connectors![0].config.triggers).toEqual(["new_issue", "release"]);
  expect(stored.notification_connectors![1]).toEqual(chat);
  view.unmount();
  render(<NotificationsPage settings={stored} onSave={save} />);
  openConnector("Older selection");
  expect(checked(screen.getByRole("checkbox", {name:"Release"}))).toBe(true);
});
