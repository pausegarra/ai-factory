import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { noticeSettings, noticeSnapshot } from "../../../../fixtures/notices";
import { usage } from "../../../../fixtures/usage";
import { makeDetail } from "../../../../fixtures/run-detail";
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true, factoryApi: { settings: vi.fn(), snapshot: vi.fn(), subscribe: vi.fn(async () => () => {}),
    agentUsage: vi.fn(), agentAccounts: vi.fn(async () => []), saveSettings: vi.fn(), detail: vi.fn(), logs: vi.fn() },
}));
import { factoryApi } from "../../../../../src/context/factory/infrastructure/ipc/factory-api";
import { App } from "../../../../../src/context/factory/infrastructure/presentations/app";
beforeEach(() => {
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
  vi.mocked(factoryApi.settings).mockResolvedValue(noticeSettings()); vi.mocked(factoryApi.snapshot).mockResolvedValue(noticeSnapshot());
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => usage(agent));
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });
it("loads only on opening, keeps navigation order, places usage between engine and footer and restores focus", async () => {
  const user = userEvent.setup(); const { container } = render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  expect(factoryApi.agentUsage).not.toHaveBeenCalled();
  expect(within(screen.getByRole("navigation")).getAllByRole("button").map(button => button.textContent)).toEqual(["Overview", "Runs", "Workflows", "Settings"]);
  const trigger = screen.getByRole("button", { name: "View Usage" });
  expect(trigger.getAttribute("aria-haspopup")).toBe("dialog"); expect(trigger.getAttribute("title")).toBe("View Usage");
  const bottom = container.querySelector(".sidebar-bottom")!;
  expect([...bottom.children].map(child => child.className)).toEqual(["engine-status", "nav-item usage-trigger", "sidebar-footer"]);
  await user.click(trigger); await screen.findByRole("dialog", { name: "Agent usage" });
  expect(container.querySelector("aside")?.hasAttribute("inert")).toBe(true);
  expect(container.querySelector(".workspace")?.hasAttribute("inert")).toBe(true);
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  expect(container.querySelector("aside")?.hasAttribute("inert")).toBe(false);
  expect(screen.getByRole("heading", { name: "Factory overview." })).toBeTruthy();
});
it("preserves a real Settings draft and tab, then still protects unsaved navigation", async () => {
  const user = userEvent.setup(); render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  await user.click(screen.getByRole("button", { name: "Settings" }));
  const field = screen.getByLabelText("Codex CLI") as HTMLInputElement;
  await user.clear(field); await user.type(field, "/fixture/codex-draft");
  await user.click(screen.getByRole("button", { name: "View Usage" }));
  await screen.findByText("Remaining: 73%"); await user.click(screen.getByRole("button", { name: "Close usage" }));
  expect((screen.getByLabelText("Codex CLI") as HTMLInputElement).value).toBe("/fixture/codex-draft");
  expect(screen.getByRole("tab", { name: "General" }).getAttribute("aria-selected")).toBe("true");
  expect(factoryApi.saveSettings).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Overview" }));
  expect(screen.getByRole("alertdialog", { name: "Your work is still here" })).toBeTruthy();
  expect((screen.getByRole("button", { name: "View Usage" }) as HTMLButtonElement).disabled).toBe(true);
});
it("keeps the selected run mounted when usage opens and closes", async () => {
  const detail = makeDetail(); const snapshot = noticeSnapshot(); snapshot.runs = [detail.run];
  vi.mocked(factoryApi.snapshot).mockResolvedValue(snapshot);
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  vi.mocked(factoryApi.logs).mockResolvedValue({ text: "", offset: 0, next_offset: 0, eof: true });
  const user = userEvent.setup(); render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  await user.click(screen.getByText("[AI] Run step order"));
  await screen.findByRole("heading", { name: "[AI] Run step order" });
  const calls = vi.mocked(factoryApi.detail).mock.calls.length;
  await user.click(screen.getByRole("button", { name: "View Usage" })); await screen.findByText("Remaining: 73%");
  await user.keyboard("{Escape}");
  expect(screen.getByRole("heading", { name: "[AI] Run step order" })).toBeTruthy();
  expect(factoryApi.detail).toHaveBeenCalledTimes(calls);
});
