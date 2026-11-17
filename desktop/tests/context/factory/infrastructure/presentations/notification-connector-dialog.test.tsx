import { StrictMode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { NotificationsPage } from "../../../../../src/context/factory/infrastructure/presentations/notifications";
import { NotificationConnectorDialog } from "../../../../../src/context/factory/infrastructure/presentations/notification-connector-dialog";
import { newGoogleChatConnector, newTelegramConnector } from "../../../../../src/context/factory/domain/notification-connectors";
import type { Settings } from "../../../../../src/context/factory/domain/types";
import { noticeSettings } from "../../../../fixtures/notices";
import { applyConnector, cancelConnector, chooseConnector, openConnector } from "../../../../fixtures/notification-editing";

afterEach(cleanup);
const initial = (): Settings => ({...noticeSettings(), notification_connectors: [newTelegramConnector("a")]});
const value = (label: string) => (screen.getByLabelText(label) as HTMLInputElement).value;

it("contains all keyboard controls in StrictMode without resetting focus during edits", async () => {
  const user = userEvent.setup();
  render(<StrictMode><NotificationsPage settings={initial()} onSave={vi.fn()} /></StrictMode>);
  const row = screen.getByRole("button", {name: "Edit connector Telegram"}); row.focus();
  await user.keyboard("{Enter}");
  const dialog = screen.getByRole("dialog", {name: "Edit connector"});
  const close = within(dialog).getByRole("button", {name: "Close connector editor"});
  expect(dialog.getAttribute("aria-modal")).toBe("true");
  expect(row.getAttribute("aria-controls")).toBe(dialog.id);
  expect(row.getAttribute("aria-expanded")).toBe("true");
  expect(document.activeElement).toBe(close);
  await user.tab({shift: true}); expect(document.activeElement).toBe(screen.getByRole("button", {name: "Apply changes"}));
  await user.tab(); expect(document.activeElement).toBe(close);
  await user.tab(); expect(document.activeElement).toBe(screen.getByLabelText("Name"));
  await user.type(screen.getByLabelText("Name"), " draft");
  expect(document.activeElement).toBe(screen.getByLabelText("Name"));
  await Promise.resolve(); expect(document.activeElement).toBe(screen.getByLabelText("Name"));
  screen.getByRole("button", {name: "Add connector"}).focus(); expect(dialog.contains(document.activeElement)).toBe(true);
  await user.keyboard("{Escape}"); await waitFor(() => expect(document.activeElement).toBe(row));
  await user.tab(); expect(document.activeElement).not.toBe(close); expect(screen.queryByRole("dialog")).toBeNull();
  row.focus(); await user.keyboard(" "); expect(screen.getByRole("dialog")).toBeTruthy();
});

it.each(["Cancel", "Close connector editor", "Escape"])("discards only the current edit with %s and restores the renamed row", async method => {
  const user = userEvent.setup(), dirty = vi.fn(), onSave = vi.fn();
  render(<NotificationsPage settings={initial()} onSave={onSave} onDirtyChange={dirty} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Applied"}}); applyConnector();
  openConnector("Applied"); fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Lost"}});
  if (method === "Escape") await user.keyboard("{Escape}"); else await user.click(screen.getByRole("button", {name: method}));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Edit connector Applied"})));
  expect(dirty).toHaveBeenLastCalledWith(true); expect(onSave).not.toHaveBeenCalled();
  openConnector("Applied"); expect(value("Name")).toBe("Applied");
  expect(screen.getByRole("region", {name: "Notifications"}).hasAttribute("inert")).toBe(true);
  for (const name of ["Add connector", "Save changes", "Discard changes", "Edit connector Applied"]) expect(screen.getByRole("button", {name}).matches(":disabled")).toBe(true);
});

it.each(["Telegram", "Google Chat"] as const)("transfers focus once from picker to new %s editor and restores Add on cancel", async label => {
  const user = userEvent.setup(), dirty = vi.fn();
  render(<StrictMode><NotificationsPage settings={noticeSettings()} onSave={vi.fn()} onDirtyChange={dirty} /></StrictMode>);
  const dialog = chooseConnector(label), close = within(dialog).getByRole("button", {name: "Close connector editor"});
  await Promise.resolve(); expect(document.activeElement).toBe(close); expect(screen.getAllByRole("dialog")).toHaveLength(1);
  await user.keyboard("{Escape}"); await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Add connector"})));
  expect(dirty).not.toHaveBeenCalledWith(true); expect(screen.queryByRole("list")).toBeNull();
});

it("disables every editor control and ignores Escape while busy", async () => {
  const user = userEvent.setup(), cancel = vi.fn(), apply = vi.fn(), remove = vi.fn();
  render(<NotificationConnectorDialog id="busy-editor" connector={newGoogleChatConnector("a")} sources={[]} mode="existing" attempted={false} busy
    onChange={vi.fn()} onCancel={cancel} onApply={apply} onRemove={remove} returnFocus={() => null} />);
  for (const control of screen.getByRole("dialog").querySelectorAll("button, input, select")) expect(control.matches(":disabled")).toBe(true);
  fireEvent.keyDown(screen.getByRole("dialog"), {key: "Escape"}); await user.click(screen.getByRole("button", {name: "Cancel"}));
  expect(cancel).not.toHaveBeenCalled(); expect(apply).not.toHaveBeenCalled(); expect(remove).not.toHaveBeenCalled();
});

it("releases focus listeners and restores a connected main when the opener unmounts", async () => {
  const view = render(<><main tabIndex={-1}>Fallback</main><NotificationsPage settings={initial()} onSave={vi.fn()} /></>);
  openConnector("Telegram");
  view.rerender(<main tabIndex={-1}>Fallback</main>);
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("main")));
  const button = document.createElement("button"); document.body.append(button); button.focus();
  expect(document.activeElement).toBe(button); button.remove();
});

it.each([newTelegramConnector, newGoogleChatConnector])("defers clean incoming settings during secret edits and adopts them on cancel (%#)", create => {
  const c = create("a"), secret = c.type === "telegram" ? "Bot token" : "Webhook URL";
  const original = {...noticeSettings(), notification_connectors: [c]}, dirty = vi.fn(), onSave = vi.fn();
  const view = render(<NotificationsPage settings={original} onSave={onSave} onDirtyChange={dirty} />);
  openConnector(c.label); fireEvent.change(screen.getByLabelText(secret), {target: {value: "PRIVATE_LOCAL"}});
  const incoming = {...original, notification_connectors: [{...c, label: "Incoming", config: {...c.config, has_token: true, has_webhook_url: true, bot_token: "PRIVATE_REMOTE", webhook_url: "PRIVATE_REMOTE"}}]};
  view.rerender(<NotificationsPage settings={incoming} onSave={onSave} onDirtyChange={dirty} />);
  expect(value("Name")).toBe(c.label); expect(value(secret)).toBe("PRIVATE_LOCAL"); expect(document.body.innerHTML).not.toContain("PRIVATE_REMOTE");
  expect(screen.getByText(c.type === "telegram" ? "Token configured" : "Webhook configured")).toBeTruthy();
  cancelConnector(); expect(screen.getByRole("button", {name: "Edit connector Incoming"})).toBeTruthy();
  expect(dirty).toHaveBeenLastCalledWith(false);
  openConnector("Incoming"); expect(value(secret)).toBe(""); expect(onSave).not.toHaveBeenCalled();
});

it("adopts refreshed values into an untouched editor and closes it on external removal", async () => {
  const original = initial(), onSave = vi.fn();
  const view = render(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector("Telegram");
  view.rerender(<NotificationsPage settings={{...original, notification_connectors: [{...newTelegramConnector("a"), label: "Incoming"}]}} onSave={onSave} />);
  expect(value("Name")).toBe("Incoming");
  view.rerender(<NotificationsPage settings={noticeSettings()} onSave={onSave} />);
  expect(screen.queryByRole("dialog")).toBeNull();
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Add connector"})));
});

it("retains the applied page draft and refreshed hints when a later session is cancelled", () => {
  const original = initial(), onSave = vi.fn();
  const view = render(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Applied"}});
  fireEvent.change(screen.getByLabelText("Bot token"), {target: {value: "PRIVATE_APPLIED"}}); applyConnector();
  openConnector("Applied"); fireEvent.change(screen.getByLabelText("Bot token"), {target: {value: "PRIVATE_CANCELLED"}});
  const incoming = {...newTelegramConnector("a"), label: "Remote", config: {...newTelegramConnector("a").config, has_token: true}};
  view.rerender(<NotificationsPage settings={{...original, notification_connectors: [incoming]}} onSave={onSave} />);
  cancelConnector(); openConnector("Applied"); expect(value("Bot token")).toBe("PRIVATE_APPLIED"); expect(screen.getByText("Token configured")).toBeTruthy();
  cancelConnector(); fireEvent.click(screen.getByRole("button", {name: "Discard changes"}));
  openConnector("Remote"); expect(value("Bot token")).toBe("");
});

it("opens only the first invalid connector, keeps error focus and saves the corrected set once", async () => {
  const invalid = {...newTelegramConnector("b"), label: "Invalid", enabled: true}, onSave = vi.fn(async (s: Settings) => s);
  const settings = {...initial(), notification_connectors: [newTelegramConnector("a"), invalid]};
  render(<main tabIndex={-1}><NotificationsPage settings={settings} onSave={onSave} /></main>);
  openConnector("Invalid"); fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Changed"}}); applyConnector();
  fireEvent.click(screen.getByRole("button", {name: "Save changes"}));
  expect(screen.getAllByRole("dialog")).toHaveLength(1); expect(value("Name")).toBe("Changed"); expect(onSave).not.toHaveBeenCalled();
  await Promise.resolve(); expect(document.activeElement).toBe(screen.getByLabelText("Chat ID"));
  expect(within(screen.getByRole("dialog")).getByRole("alert").textContent).toContain("Chat ID");
  fireEvent.click(screen.getByRole("checkbox", {name: "Enabled"})); applyConnector();
  fireEvent.click(screen.getByRole("button", {name: "Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave).toHaveBeenCalledTimes(1); expect(onSave.mock.calls[0][0].notification_connectors?.map(c => c.id)).toEqual(["a", "b"]);
});

it("never shows destinations or credential presence in list rows", () => {
  const c = newTelegramConnector("a"); c.config.chat_id = "PRIVATE_CHAT"; c.config.has_token = true; c.config.bot_token = "PRIVATE_TOKEN";
  const chat = newGoogleChatConnector("b"); chat.config.has_webhook_url = true; chat.config.webhook_url = "https://private.test/PRIVATE_URL";
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [c, chat]}} onSave={vi.fn()} />);
  const list = screen.getByRole("list"); expect(list.textContent).toBe("TelegramGoogle Chat");
  expect(list.querySelectorAll("input, select, fieldset")).toHaveLength(0);
  expect(document.body.innerHTML).not.toMatch(/PRIVATE_CHAT|PRIVATE_TOKEN|PRIVATE_URL|Token configured|Webhook configured/);
  const rows = within(list).getAllByRole("button"); expect(rows[0].querySelector("svg")).toBeTruthy(); expect(rows[1].querySelector("img")?.getAttribute("alt")).toBe("");
});

it.each([newTelegramConnector, newGoogleChatConnector])("retains a modal replacement through a credential refresh before apply (%#)", async create => {
  const c = create("a"), onSave = vi.fn(async (s: Settings) => s);
  const secret = c.type === "telegram" ? "Bot token" : "Webhook URL";
  const replacement = c.type === "telegram" ? "fixture_local_replacement" : "https://chat.googleapis.com/v1/spaces/fixture/messages?key=LOCAL&token=FIXTURE";
  const original = {...noticeSettings(), notification_connectors: [c]};
  const view = render(<NotificationsPage settings={original} onSave={onSave} />);
  openConnector(c.label); fireEvent.change(screen.getByLabelText(secret), {target: {value: replacement}});
  const incoming = {...c, config: {...c.config, ...(c.type === "telegram" ? {has_token: true} : {has_webhook_url: true})}};
  view.rerender(<NotificationsPage settings={{...original, max_concurrent: 9, notification_connectors: [incoming]}} onSave={onSave} />);
  expect(value(secret)).toBe(replacement);
  expect(screen.getByText(c.type === "telegram" ? "Token configured" : "Webhook configured")).toBeTruthy();
  applyConnector(); fireEvent.click(screen.getByRole("button", {name: "Save changes"})); await screen.findByText("Notification settings saved.");
  expect(onSave).toHaveBeenCalledTimes(1); expect(onSave.mock.calls[0][0].max_concurrent).toBe(9);
  expect(onSave.mock.calls[0][0].notification_connectors![0].config[c.type === "telegram" ? "bot_token" : "webhook_url"]).toBe(replacement);
  openConnector(c.label); expect(value(secret)).toBe("");
});

it.each(["Telegram", "Google Chat"] as const)("cancels new %s configuration while retaining prior dirty changes", label => {
  const dirty = vi.fn(), onSave = vi.fn();
  render(<NotificationsPage settings={initial()} onSave={onSave} onDirtyChange={dirty} />);
  openConnector("Telegram"); fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Applied"}}); applyConnector();
  chooseConnector(label); fireEvent.change(screen.getByLabelText("Name"), {target: {value: "Temporary"}}); cancelConnector();
  expect(within(screen.getByRole("list")).getAllByRole("button")).toHaveLength(1);
  expect(screen.getByRole("button", {name: "Edit connector Applied"})).toBeTruthy(); expect(dirty).toHaveBeenLastCalledWith(true);
  expect(onSave).not.toHaveBeenCalled();
});

it("restores a neighboring row by identity after middle and last removals", async () => {
  render(<NotificationsPage settings={{...noticeSettings(), notification_connectors: [
    newTelegramConnector("a"), {...newTelegramConnector("b"), label: "Middle"}, {...newTelegramConnector("c"), label: "Last"},
  ]}} onSave={vi.fn()} />);
  openConnector("Middle"); fireEvent.click(screen.getByRole("button", {name: "Remove connector Middle"}));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Edit connector Last"})));
  openConnector("Last"); fireEvent.click(screen.getByRole("button", {name: "Remove connector Last"}));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Edit connector Telegram"})));
});
