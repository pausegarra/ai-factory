import { it, expect, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => true,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import type { Settings } from "../src/context/factory/domain/types";
it("deletes exactly one run using the host argument contract", async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ state: "deleted", run_id: "run-1" });
  await expect(factoryApi.deleteRun("run-1")).resolves.toEqual({ state: "deleted", run_id: "run-1" });
  expect(invoke).toHaveBeenCalledWith("delete_run", { runId: "run-1" });
});
it("sends run and node identifiers using host argument contract", async () => {
  await factoryApi.logs("run-1", "plan", 128);
  expect(invoke).toHaveBeenCalledWith("get_logs", {
    runId: "run-1",
    nodeId: "plan",
    offset: 128,
  });
  await factoryApi.cancel("run-1");
  expect(invoke).toHaveBeenCalledWith("cancel_run", { runId: "run-1" });
});
it("preserves profile activation through existing settings IPC commands", async () => {
  const settings = { flows: [{ id: "new_issue", enabled: false, profile_activation: JSON.parse('{"work":false,"constructor":true,"__proto__":false}'), nodes: [] }] } as unknown as Settings;
  vi.mocked(invoke).mockResolvedValue(settings);
  expect(await factoryApi.settings()).toEqual(settings);
  expect(invoke).toHaveBeenCalledWith("get_settings");
  expect(await factoryApi.saveSettings(settings)).toEqual(settings);
  expect(invoke).toHaveBeenCalledWith("save_settings", { settings });
  expect(Object.keys((await factoryApi.settings()).flows[0].profile_activation!)).toEqual(["work", "constructor", "__proto__"]);
});
it("requests the catalog for the selected agent", async () => {
  await factoryApi.agentModels("codex");
  expect(invoke).toHaveBeenCalledWith("get_agent_models", {agent: "codex"});
});
it("uses account IDs for managed OAuth commands and scopes model catalogs by GitHub profile", async () => {
  await factoryApi.agentAccounts();
  expect(invoke).toHaveBeenCalledWith("get_agent_accounts", undefined);
  await factoryApi.startCodexLogin("Work");
  expect(invoke).toHaveBeenCalledWith("start_codex_login", { label: "Work" });
  await factoryApi.openCodexLogin("account-1");
  expect(invoke).toHaveBeenCalledWith("open_codex_login", { accountId: "account-1" });
  await factoryApi.removeAgentAccount("account-1");
  expect(invoke).toHaveBeenCalledWith("remove_agent_account", { accountId: "account-1" });
  await factoryApi.agentModels("codex", "work");
  expect(invoke).toHaveBeenCalledWith("get_agent_models", { agent: "codex", tokenProfileId: "work" });
});
it("passes notification secrets only in explicit save input through existing IPC", async () => {
  const input = {notification_connectors:[{id:"team",label:"Team",enabled:true,events:["run_failed"],type:"telegram",config:{chat_id:"-1001",bot_token:"synthetic_fixture_token",has_token:false}}]} as unknown as Settings;
  const output = {notification_connectors:[{...input.notification_connectors![0],config:{chat_id:"-1001",has_token:true}}]} as unknown as Settings;
  vi.mocked(invoke).mockResolvedValue(output);
  const saved = await factoryApi.saveSettings(input);
  expect(invoke).toHaveBeenCalledWith("save_settings",{settings:input});
  expect(saved.notification_connectors?.[0].config.bot_token).toBeUndefined();
  const loaded = await factoryApi.settings();
  expect(loaded.notification_connectors?.[0].config).toEqual({chat_id:"-1001",has_token:true});
});
it("transports independent ceilings and reserved capacity through existing IPC", async () => {
  const { noticeSettings, noticeSnapshot } = await import("./fixtures/notices");
  const settings = { ...noticeSettings(), max_concurrent: 8, trigger_max_concurrent: { new_issue: 4, pr_review: 4, pr_approved: 2 } };
  vi.mocked(invoke).mockResolvedValueOnce(settings);
  expect((await factoryApi.settings()).trigger_max_concurrent).toEqual({ new_issue: 4, pr_review: 4, pr_approved: 2 });
  vi.mocked(invoke).mockResolvedValueOnce(settings);
  expect((await factoryApi.saveSettings(settings)).trigger_max_concurrent).toEqual({ new_issue: 4, pr_review: 4, pr_approved: 2 });
  expect(invoke).toHaveBeenLastCalledWith("save_settings", { settings });
  const snapshot = noticeSnapshot();
  snapshot.capacity = { global: { active: 1, limit: 8 }, by_trigger: {
    new_issue: { active: 0, limit: 4 }, pr_review: { active: 0, limit: 4 }, pr_approved: { active: 1, limit: 2 },
  } };
  vi.mocked(invoke).mockResolvedValueOnce(snapshot);
  expect((await factoryApi.snapshot()).capacity).toEqual(snapshot.capacity);
  expect(invoke).toHaveBeenLastCalledWith("get_snapshot");
});
