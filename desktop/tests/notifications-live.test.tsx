import { openConnector, applyConnector, cancelConnector } from "./fixtures/notification-editing";
import { beforeAll, afterEach, beforeEach, it, expect, vi } from "vitest";
import {render, screen, cleanup, fireEvent, waitFor, act, within} from "@testing-library/react";
import type {FactoryEvent, Settings} from "../src/context/factory/domain/types";
import {noticeSettings, noticeSnapshot, deferred} from "./fixtures/notices";
import {newTelegramConnector} from "../src/context/factory/domain/notification-connectors";
const mock=vi.hoisted(()=>({listener:null as null | ((event:FactoryEvent)=>void)}));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api",()=>({native:true,factoryApi:{
  settings:vi.fn(), snapshot:vi.fn(), saveSettings:vi.fn(),
  subscribe:vi.fn(async (listener:(e:FactoryEvent)=>void)=>{mock.listener=listener;return ()=>{mock.listener=null;};}),
}}));
import {factoryApi} from "../src/context/factory/infrastructure/ipc/factory-api";
import {App} from "../src/context/factory/infrastructure/presentations/app";
beforeAll(()=>{globalThis.ResizeObserver=class {observe() {} unobserve() {} disconnect() {}} as unknown as typeof ResizeObserver;});
beforeEach(()=>{
  vi.mocked(factoryApi.settings).mockResolvedValue({...noticeSettings(), notification_connectors:[newTelegramConnector("a")]});
  vi.mocked(factoryApi.snapshot).mockResolvedValue(noticeSnapshot());
  vi.mocked(factoryApi.saveSettings).mockImplementation(async s=>structuredClone(s));
});
afterEach(()=>{cleanup();vi.clearAllMocks();});
it("notifications navigation preserves Settings and warns before discarding edits",async()=>{
  render(<App />);await screen.findByRole("heading",{name:/Factory overview/});
  fireEvent.click(screen.getByRole("button",{name:"Notifications"}));
  await screen.findByRole("heading",{name:"Notifications"});
  expect(screen.getByRole("button",{name:"Notifications"}).getAttribute("aria-current")).toBe("page");
  fireEvent.click(screen.getByRole("button",{name:"Settings"}));await screen.findByRole("heading",{name:"Settings"});
  fireEvent.click(screen.getByRole("button",{name:"Notifications"}));
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Local edit"}});
  applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Runs"}));expect(screen.getByRole("alertdialog")).toBeTruthy();
  fireEvent.click(screen.getByRole("button",{name:"Keep editing"}));openConnector("Local edit");expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Local edit");
  cancelConnector(); fireEvent.click(screen.getByRole("button",{name:"Runs"}));fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button",{name:"Discard changes"}));
  await screen.findByRole("heading",{name:"Runs"});expect(factoryApi.saveSettings).not.toHaveBeenCalled();
});
it("ignores a live Settings read started before save and accepts a later refresh",async()=>{
  const profiles=[{id:"work",label:"Work",account:"alice",has_token:true,owners:[],authors:[],reviewers:[]}];
  vi.mocked(factoryApi.settings).mockResolvedValue({...noticeSettings(),token_profiles:profiles,notification_connectors:[newTelegramConnector("a")]});
  render(<App />);await screen.findByRole("heading",{name:/Factory overview/});
  fireEvent.click(screen.getByRole("button",{name:"Notifications"}));
  const stale=deferred<Settings>();vi.mocked(factoryApi.settings).mockReturnValueOnce(stale.promise);
  act(()=>mock.listener?.({kind:"settings_changed",timestamp:"now"}));
  await waitFor(()=>expect(factoryApi.settings).toHaveBeenCalledTimes(2));
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Saved label"}});
  fireEvent.click(screen.getByRole("checkbox",{name:"New task"}));
  fireEvent.click(screen.getByRole("checkbox",{name:"Work (@alice)"}));
  applyConnector(); fireEvent.click(screen.getByRole("button",{name:"Save changes"}));await screen.findByText("Notification settings saved.");
  await act(async()=>{stale.resolve({...noticeSettings(),notification_connectors:[newTelegramConnector("a")]});await stale.promise;});
  openConnector("Saved label"); expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Saved label");
  expect((screen.getByRole("checkbox",{name:"New task"}) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("checkbox",{name:"Work (@alice)"}) as HTMLInputElement).checked).toBe(true);
  expect(vi.mocked(factoryApi.saveSettings).mock.calls[0][0].notification_connectors![0].config).toMatchObject({triggers:["pr_approved","pr_review","release"],token_profile_ids:["work"]});
  cancelConnector(); const latest={...noticeSettings(),token_profiles:profiles,notification_connectors:[{...newTelegramConnector("a"),label:"Later refresh"}]};
  vi.mocked(factoryApi.settings).mockResolvedValue(latest);
  act(()=>mock.listener?.({kind:"settings_changed",timestamp:"later"}));
  await screen.findByRole("button",{name:"Edit connector Later refresh"}); openConnector("Later refresh");
  expect((screen.getByRole("checkbox",{name:"New task"}) as HTMLInputElement).checked).toBe(true);
  expect((screen.getByRole("checkbox",{name:"Work (@alice)"}) as HTMLInputElement).checked).toBe(false);
});

it("cancelling modal edits leaves navigation clean", async () => {
  render(<App />); await screen.findByRole("heading",{name:/Factory overview/});
  fireEvent.click(screen.getByRole("button",{name:"Notifications"})); openConnector("Telegram");
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Cancelled"}}); cancelConnector();
  fireEvent.click(screen.getByRole("button",{name:"Runs"})); await screen.findByRole("heading",{name:"Runs"});
  expect(screen.queryByRole("alertdialog")).toBeNull(); expect(factoryApi.saveSettings).not.toHaveBeenCalled();
});
