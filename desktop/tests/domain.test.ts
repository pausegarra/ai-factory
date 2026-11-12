import { describe, it, expect } from "vitest";
import {
  validateFlow,
  connectNodes,
  removeNode,
  validateSettings,
  activationStatus,
  profilePreference,
} from "../src/context/factory/domain/rules";
import type { Flow, Settings } from "../src/context/factory/domain/types";
const node = (id: string, depends_on: string[] = []) => ({
  id,
  name: id,
  agent: "codex" as const,
  model: "gpt-5.1-codex",
  effort: "medium",
  access: "write" as const,
  prompt: "Do work",
  result_schema: {},
  depends_on,
  position: { x: 0, y: 0 },
});
const flow = (): Flow => ({
  id: "new_issue",
  name: "Issue",
  enabled: true,
  nodes: [node("plan"), node("build", ["plan"])],
});
describe("workflow editing", () => {
  it.each([true, false])("gives global activation %s precedence over account preferences", (enabled) => {
    for (const preference of [true, false]) {
      const value = { ...flow(), enabled, profile_activation: { work: preference } };
      expect(profilePreference(value, "work")).toBe(preference);
      expect(activationStatus(value, "work")).toBe(!enabled ? "Disabled globally" : preference ? "Enabled" : "Disabled for this account");
    }
  });
  it("defaults absent preferences and ignores inherited properties", () => {
    const value = flow();
    expect(profilePreference(value, "constructor")).toBe(true);
    value.profile_activation = Object.create({ work: false });
    expect(profilePreference(value, "work")).toBe(true);
    value.profile_activation = JSON.parse('{"constructor":false,"__proto__":false}');
    expect(profilePreference(value, "constructor")).toBe(false);
    expect(profilePreference(value, "__proto__")).toBe(false);
    expect(validateFlow(value)).toEqual([]);
  });
  it.each([null, [], false, "bad", { work: "false" }, { "../bad": false }, { ".": true }, { "..": false }, { "": true }])("rejects malformed activation map %j", (activation) => {
    expect(validateFlow({ ...flow(), profile_activation: activation } as unknown as Flow).join(" ")).toMatch(/activation/i);
  });
  it("rejects non boolean global activation", () => {
    expect(validateFlow({ ...flow(), enabled: "false" } as unknown as Flow)).toContain("Workflow activation must be a boolean.");
  });
  it("validates optional per-token agent configurations", () => {
    const value = flow();
    value.nodes[0].token_overrides = { work: { agent: "opencode", model: "", effort: "high" } };
    expect(validateFlow(value).join(" ")).toMatch(/choose a model.*token work/i);
    value.nodes[0].token_overrides.work.model = "provider/model";
    expect(validateFlow(value)).toEqual([]);
    value.nodes[0].token_overrides.work = { agent: "opencode", model: "", effort: "" };
    expect(validateFlow(value)).toEqual([]);
    value.nodes[0].token_overrides["../invalid"] = { agent: "codex", model: "", effort: "" };
    expect(validateFlow(value).join(" ")).toMatch(/invalid token override/i);
  });
  it("rejects cycle when connecting a descendant to an ancestor", () => {
    expect(() => connectNodes(flow(), "build", "plan")).toThrow(/cycle/i);
  });
  it("removes deleted nodes from all dependencies", () => {
    const result = removeNode(flow(), "plan");
    expect(result.nodes).toHaveLength(1);
    expect(result.nodes[0].depends_on).toEqual([]);
  });
  it("rejects missing dependencies and non object schemas", () => {
    const value = flow();
    value.nodes[0].depends_on = ["missing"];
    expect(validateFlow(value).join(" ")).toMatch(/does not exist/i);
    value.nodes[0].depends_on = [];
    value.nodes[0].result_schema = [];
    expect(validateFlow(value).join(" ")).toMatch(/object/i);
  });
  it("allows independent nodes and prevents duplicate connections", () => {
    expect(validateFlow(flow())).toEqual([]);
    expect(connectNodes(flow(), "plan", "build").nodes[1].depends_on).toEqual([
      "plan",
    ]);
  });
});
describe("settings validation", () => {
  it.each(["", "old/relative/path"])("accepts unused legacy project path %j", (source_path) => {
    const settings = {
      max_concurrent: 4,
      poll_interval_seconds: 10,
      commands: { codex: "codex", opencode: "opencode", git: "git" },
      token_profiles: [],
      projects: [{ id: "legacy", repository: "owner/repo", source_path }],
      flows: [],
    } as unknown as Settings;

    expect(validateSettings(settings)).toEqual([]);
  });

  it("rejects unsafe concurrency and empty token secrets for new profiles", () => {
    const settings = {
      max_concurrent: 0,
      poll_interval_seconds: 10,
      commands: { codex: "codex", opencode: "opencode", git: "git" },
      token_profiles: [
        { id: "a", label: "Work", account: "me", has_token: false },
      ],
      projects: [],
      flows: [],
    } as unknown as Settings;
    expect(validateSettings(settings).join(" ")).toMatch(/concurrency/i);
    expect(validateSettings(settings).join(" ")).toMatch(/token/i);
  });
});
