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
it("reports usage as desktop-only without invoking or inspecting local accounts", async () => {
  for (const agent of ["codex", "opencode"] as const) {
    const result = await factoryApi.agentUsage(agent);
    expect(result.agent).toBe(agent);
    expect(result.status).toBe("preview");
    expect(result.code).toBe("desktop_required");
    expect(result.entries).toEqual([]);
  }
  expect(invoke).not.toHaveBeenCalled();
});
it("loads empty notification preferences and refuses preview writes without invoking native commands", async () => {
  const settings = await factoryApi.settings();
  expect(settings.notification_connectors).toEqual([]);
  await expect(factoryApi.saveSettings(settings)).rejects.toThrow(/desktop/);
  expect(invoke).not.toHaveBeenCalled();
});
it("exposes empty capacity and inherited trigger limits in preview", async () => {
  const settings = await factoryApi.settings();
  const snapshot = await factoryApi.snapshot();
  expect(settings.trigger_max_concurrent).toEqual({});
  expect(snapshot.capacity.global).toEqual({ active: 0, limit: 4 });
  expect(snapshot.capacity.by_trigger).toEqual({
    new_issue: { active: 0, limit: 4 },
    pr_review: { active: 0, limit: 4 },
    pr_approved: { active: 0, limit: 4 },
  });
  snapshot.capacity.by_trigger.new_issue.active = 30;
  settings.trigger_max_concurrent!.pr_review = 1;
  expect((await factoryApi.snapshot()).capacity.by_trigger.new_issue.active).toBe(0);
  expect((await factoryApi.settings()).trigger_max_concurrent).toEqual({});
  expect(invoke).not.toHaveBeenCalled();
});

it("defaults automatic issue comments to enabled in isolated preview settings", async () => {
  const first = await factoryApi.settings();
  expect(first.issue_node_comments_enabled).toBe(true);
  first.issue_node_comments_enabled = false;
  expect((await factoryApi.settings()).issue_node_comments_enabled).toBe(true);
});
