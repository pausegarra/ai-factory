import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { SettingsPage } from "../src/context/factory/infrastructure/presentations/settings";
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import type { AgentAccount, Settings } from "../src/context/factory/domain/types";

vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: { agentAccounts: vi.fn(), startCodexLogin: vi.fn(), openCodexLogin: vi.fn(), removeAgentAccount: vi.fn() },
}));
const account = (id = "codex-work", status: AgentAccount["status"] = "ready"): AgentAccount => ({ id, agent: "codex", label: "Work Codex", email: status === "ready" ? "person@example.com" : null, status, error: null });
const settings = (): Settings => ({
  token_profiles: [{ id: "work", label: "Work", account: "worker", has_token: true, owners: [], authors: [], reviewers: [] }],
  projects: [], commands: { codex: "codex", opencode: "opencode", git: "git" },
  poll_interval_seconds: 15, max_concurrent: 4, start_at_login: false,
  flows: [],
});
function agents() { fireEvent.click(screen.getByRole("tab", { name: "Agents" })); }
beforeEach(() => { vi.resetAllMocks(); vi.mocked(factoryApi.agentAccounts).mockResolvedValue([]); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

it("keeps Codex YOLO off by default and stages changes until Save", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={settings()} onSave={save} appDir="/app" />);
  const toggle = screen.getByRole("checkbox", { name: /^Codex YOLO mode/ }) as HTMLInputElement;
  expect(toggle.checked).toBe(false);
  expect(screen.getByText(/all Codex steps.*full filesystem and network access/i)).toBeTruthy();
  fireEvent.click(toggle);
  expect(toggle.checked).toBe(true);
  expect(save).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect(toggle.checked).toBe(false);
  fireEvent.click(toggle);
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].codex_yolo).toBe(true);
});

it("can save Codex YOLO disabled after loading enabled settings", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  const initial = { ...settings(), codex_yolo: true };
  render(<SettingsPage settings={initial} onSave={save} appDir="/app" />);
  const toggle = screen.getByRole("checkbox", { name: /^Codex YOLO mode/ }) as HTMLInputElement;
  expect(toggle.checked).toBe(true);
  fireEvent.click(toggle);
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].codex_yolo).toBe(false);
});

it("offers accessible Settings tabs and keeps agent auth outside GitHub profiles", async () => {
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  const general = screen.getByRole("tab", { name: "General" });
  expect(general.getAttribute("aria-selected")).toBe("true");
  for (const tab of screen.getAllByRole("tab")) expect(document.getElementById(tab.getAttribute("aria-controls")!)).toBeTruthy();
  expect(screen.queryByLabelText(/Codex account for/)).toBeNull();
  fireEvent.keyDown(general, { key: "ArrowRight" });
  const github = screen.getByRole("tab", { name: "GitHub accounts" });
  expect(document.activeElement).toBe(github);
  expect(screen.getByRole("tabpanel", { name: "GitHub accounts" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "GitHub accounts" })).toBeTruthy();
  fireEvent.keyDown(github, { key: "ArrowRight" });
  await screen.findByRole("heading", { name: "Codex" });
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Agents" }));
  expect(screen.getByRole("tabpanel", { name: "Agents" })).toBeTruthy();
  expect(within(screen.getByRole("region", { name: "OpenCode" })).getByText("Default CLI")).toBeTruthy();
  expect(within(screen.getByRole("region", { name: "OpenCode" })).queryByRole("button")).toBeNull();
  await screen.findByText("No Codex accounts yet");
  expect(screen.getByText(/Unassigned GitHub profiles block Codex steps/)).toBeTruthy();
  fireEvent.keyDown(screen.getByRole("tab", { name: "Agents" }), { key: "ArrowRight" });
  expect(document.activeElement).toBe(general);
  expect(screen.queryByRole("heading", { name: "GitHub accounts" })).toBeNull();
  fireEvent.keyDown(general, { key: "ArrowLeft" });
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Agents" }));
});

it("stages profile assignments until Save changes and preserves draft across unrelated updates", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account()]);
  const save = vi.fn().mockResolvedValue(undefined);
  const initial = settings();
  const view = render(<SettingsPage settings={initial} onSave={save} appDir="/app" />);
  agents();
  await screen.findByRole("option", { name: /Work Codex/ });
  fireEvent.change(screen.getByLabelText("Codex account for Work (@worker)"), { target: { value: "codex-work" } });
  expect(save).not.toHaveBeenCalled();
  view.rerender(<SettingsPage settings={{ ...initial, flows: [{ id: "new_issue", name: "Updated", enabled: false, nodes: [] }] }} onSave={save} appDir="/app" />);
  expect((screen.getByLabelText("Codex account for Work (@worker)") as HTMLSelectElement).value).toBe("codex-work");
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].codex_bindings).toEqual({ work: "codex-work" });
  expect(save.mock.calls[0][0].flows[0].name).toBe("Updated");
});

it("starts labeled OAuth through native opener and reloads pending account to ready", async () => {
  vi.mocked(factoryApi.startCodexLogin).mockResolvedValue({ account: account("new", "pending"), auth_url: "https://auth.example.test/secret" });
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("No Codex accounts yet");
  fireEvent.change(screen.getByLabelText("Account label"), { target: { value: "Work Codex" } });
  fireEvent.click(screen.getByRole("button", { name: "Add Codex account" }));
  await waitFor(() => expect(factoryApi.openCodexLogin).toHaveBeenCalledWith("new"));
  expect(factoryApi.startCodexLogin).toHaveBeenCalledWith("Work Codex");
  expect(screen.getByText("Pending")).toBeTruthy();
  expect(screen.queryByText(/auth.example/)).toBeNull();
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account("new")]);
  fireEvent.click(screen.getByRole("button", { name: "Reload accounts" }));
  await screen.findByText("person@example.com");
  expect(screen.getByText("Ready")).toBeTruthy();
});

it("shows OAuth failures and cancels pending accounts immediately", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([{ ...account("failed", "failed"), error: "Sign-in expired" }, account("pending", "pending")]);
  vi.mocked(factoryApi.removeAgentAccount).mockResolvedValue(undefined);
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("Sign-in expired");
  fireEvent.click(screen.getByRole("button", { name: "Cancel sign-in for Work Codex" }));
  await waitFor(() => expect(factoryApi.removeAgentAccount).toHaveBeenCalledWith("pending"));
  await waitFor(() => expect(screen.queryByText("Pending")).toBeNull());
});

it("retains account and bindings when active-account deletion fails", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account()]);
  vi.mocked(factoryApi.removeAgentAccount).mockRejectedValue(new Error("Account is used by an active run"));
  render(<SettingsPage settings={{ ...settings(), codex_bindings: { work: "codex-work" } }} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("person@example.com");
  fireEvent.click(screen.getByRole("button", { name: "Remove account Work Codex" }));
  await screen.findByText(/Account is used by an active run/);
  expect(document.querySelector(".notice[role='alert']")?.textContent).toContain("Could not remove Codex account.");
  expect((screen.getByLabelText("Codex account for Work (@worker)") as HTMLSelectElement).value).toBe("codex-work");
  expect(screen.getByText("person@example.com")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Dismiss notice" }));
  expect(document.querySelector(".notice[role='alert']")).toBeNull();
  expect(screen.getByText("person@example.com")).toBeTruthy();
  expect((screen.getByLabelText("Codex account for Work (@worker)") as HTMLSelectElement).value).toBe("codex-work");
});

it("clears a recovered account-load error without removing account diagnostics", async () => {
  vi.mocked(factoryApi.agentAccounts)
    .mockRejectedValueOnce(new Error("fixture account load denied"))
    .mockResolvedValueOnce([{ ...account("pending", "pending"), error: "Complete sign-in in your browser" }]);
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await waitFor(() => expect(document.querySelector(".notice[role='alert']")?.textContent).toContain("fixture account load denied"));
  expect(document.querySelector(".notice[role='alert']")?.textContent).toContain("Could not load Codex accounts.");
  fireEvent.click(screen.getByRole("button", { name: "Reload accounts" }));
  await screen.findByText("Complete sign-in in your browser");
  await waitFor(() => expect(factoryApi.agentAccounts).toHaveBeenCalledTimes(2));
  expect(document.querySelector(".notice[role='alert']")).toBeNull();
  expect(screen.getByText("Complete sign-in in your browser")).toBeTruthy();
});

it("removes stale bindings when deleting a GitHub profile and supports Discard", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={{ ...settings(), codex_bindings: { work: "codex-work" } }} onSave={save} appDir="/app" />);
  fireEvent.click(screen.getByRole("tab", { name: "GitHub accounts" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete profile Work" }));
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].codex_bindings).toEqual({});
});

it("ignores account reload results made stale by cancellation", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValueOnce([account("pending", "pending")]);
  let finish!: (value: AgentAccount[]) => void;
  vi.mocked(factoryApi.agentAccounts).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("Pending");
  fireEvent.click(screen.getByRole("button", { name: "Reload accounts" }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel sign-in for Work Codex" }));
  await waitFor(() => expect(screen.queryByText("Pending")).toBeNull());
  await act(async () => { finish([account("pending", "pending")]); });
  expect(screen.queryByText("Pending")).toBeNull();
});


it("loads Codex accounts under StrictMode effect cleanup", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account()]);
  render(<StrictMode><SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" /></StrictMode>);
  agents();
  await screen.findByText("person@example.com");
});

it("polls pending auth without overlapping requests and stops after unmount", async () => {
  vi.useFakeTimers();
  vi.mocked(factoryApi.agentAccounts).mockResolvedValueOnce([account("pending", "pending")]);
  const view = render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await act(async () => {});
  expect(screen.getByText("Pending")).toBeTruthy();
  let finish!: (value: AgentAccount[]) => void;
  vi.mocked(factoryApi.agentAccounts).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  await act(async () => { vi.advanceTimersByTime(1500); });
  expect(factoryApi.agentAccounts).toHaveBeenCalledTimes(2);
  await act(async () => { vi.advanceTimersByTime(6000); });
  expect(factoryApi.agentAccounts).toHaveBeenCalledTimes(2);
  view.unmount();
  await act(async () => { finish([account("pending")]); vi.advanceTimersByTime(6000); });
  expect(factoryApi.agentAccounts).toHaveBeenCalledTimes(2);
});

it("discards assignment edits and clears bindings after account removal", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account()]);
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={{ ...settings(), codex_bindings: { work: "codex-work" } }} onSave={save} appDir="/app" />);
  agents();
  await screen.findByText("person@example.com");
  fireEvent.change(screen.getByLabelText("Codex account for Work (@worker)"), { target: { value: "" } });
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect((screen.getByLabelText("Codex account for Work (@worker)") as HTMLSelectElement).value).toBe("codex-work");
  fireEvent.click(screen.getByRole("button", { name: "Remove account Work Codex" }));
  await screen.findByText("No Codex accounts yet");
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].codex_bindings).toEqual({});
});

it("retains pending connection when native browser opening fails", async () => {
  vi.mocked(factoryApi.startCodexLogin).mockResolvedValue({ account: account("pending", "pending"), auth_url: "https://auth.example.test/secret" });
  vi.mocked(factoryApi.openCodexLogin).mockRejectedValueOnce(new Error("Browser could not open"));
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("No Codex accounts yet");
  fireEvent.change(screen.getByLabelText("Account label"), { target: { value: "Work Codex" } });
  fireEvent.click(screen.getByRole("button", { name: "Add Codex account" }));
  await screen.findByText(/Browser could not open/);
  expect(document.querySelector(".notice[role='alert']")?.textContent).toContain("Could not continue Codex sign-in.");
  expect(screen.getByText("Pending")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Continue sign-in" }));
  await waitFor(() => expect(factoryApi.openCodexLogin).toHaveBeenCalledTimes(2));
});

it("labels account creation failures and keeps the current account list", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account()]);
  vi.mocked(factoryApi.startCodexLogin).mockRejectedValueOnce(new Error("fixture OAuth start denied"));
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("person@example.com");
  fireEvent.change(screen.getByLabelText("Account label"), { target: { value: "New account" } });
  fireEvent.click(screen.getByRole("button", { name: "Add Codex account" }));
  await waitFor(() => expect(document.querySelector(".notice[role='alert']")?.textContent).toContain("Could not add Codex account."));
  expect(screen.getByText("person@example.com")).toBeTruthy();
  expect(factoryApi.openCodexLogin).not.toHaveBeenCalled();
});

it("preserves unrelated drafts when account removal finishes after switching tabs", async () => {
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([account()]);
  let finish!: () => void;
  vi.mocked(factoryApi.removeAgentAccount).mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={{ ...settings(), codex_bindings: { work: "codex-work" } }} onSave={save} appDir="/app" />);
  agents();
  await screen.findByText("person@example.com");
  fireEvent.click(screen.getByRole("button", { name: "Remove account Work Codex" }));
  fireEvent.click(screen.getByRole("tab", { name: "GitHub accounts" }));
  fireEvent.change(screen.getByLabelText("GitHub owners"), { target: { value: "next-org" } });
  expect((screen.getByRole("button", { name: "Save changes" }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => finish());
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0].codex_bindings).toEqual({});
  expect(save.mock.calls[0][0].token_profiles[0].owners).toEqual(["next-org"]);
});

it("opens initiated sign-in even if the user switches tabs before OAuth starts", async () => {
  let finish!: (value: { account: AgentAccount; auth_url: string }) => void;
  vi.mocked(factoryApi.startCodexLogin).mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  agents();
  await screen.findByText("No Codex accounts yet");
  fireEvent.change(screen.getByLabelText("Account label"), { target: { value: "Work Codex" } });
  fireEvent.click(screen.getByRole("button", { name: "Add Codex account" }));
  fireEvent.click(screen.getByRole("tab", { name: "General" }));
  await act(async () => finish({ account: account("pending", "pending"), auth_url: "https://auth.example.test" }));
  expect(factoryApi.openCodexLogin).toHaveBeenCalledWith("pending");
});
