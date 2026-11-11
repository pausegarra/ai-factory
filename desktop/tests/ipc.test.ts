import { it, expect, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => true,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
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
it("requests the catalog for the selected agent", async () => {
  await factoryApi.agentModels("codex");
  expect(invoke).toHaveBeenCalledWith("get_agent_models", {agent: "codex"});
});
