import { describe, it, expect } from "vitest";
import {
  validateFlow,
  connectNodes,
  removeNode,
  validateSettings,
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
      node_timeout_seconds: 60,
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
      node_timeout_seconds: 60,
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
