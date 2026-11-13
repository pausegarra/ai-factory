import { it, expect, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => false }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
it("does not pretend to delete in browser preview", async () => {
  await expect(factoryApi.deleteRun("run-1")).rejects.toThrow(/desktop/);
  expect(invoke).not.toHaveBeenCalled();
});
it("provides empty managed accounts in preview and requires desktop OAuth", async () => {
  await expect(factoryApi.agentAccounts()).resolves.toEqual([]);
  await expect(factoryApi.startCodexLogin("Work")).rejects.toThrow(/desktop/);
  await expect(factoryApi.openCodexLogin("account-1")).rejects.toThrow(/desktop/);
  await expect(factoryApi.removeAgentAccount("account-1")).rejects.toThrow(/desktop/);
  expect(invoke).not.toHaveBeenCalled();
});
it("loads empty notification preferences and refuses preview writes without invoking native commands", async () => {
  const settings = await factoryApi.settings();
  expect(settings.notification_connectors).toEqual([]);
  await expect(factoryApi.saveSettings(settings)).rejects.toThrow(/desktop/);
  expect(invoke).not.toHaveBeenCalled();
});
