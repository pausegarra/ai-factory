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
