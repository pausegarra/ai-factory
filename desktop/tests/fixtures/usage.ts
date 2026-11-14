import type { Agent, AgentUsage, UsageCode } from "../../src/context/factory/domain/types";
import contract from "../../../crates/factory-core/tests/fixtures/agent_usage/codex-remaining-contract.json";
export function usage(agent: Agent = "codex"): AgentUsage {
  if (agent === "codex") return structuredClone(contract.cases[0].wire) as AgentUsage;
  return { agent, status: "available", code: null, checked_at: "2026-11-13T00:00:00Z", entries: [{
    id: "opencode:console", label: "OpenCode Console", email: null, account_status: null,
    checked_at: "2026-11-13T00:00:00Z", observation: { status: "available", code: null, metrics: [{
      id: "opencode:rolling", label: "rolling", kind: "reported", derivation: null, value: 37,
      unit: "percent", period: null, resets_at: null, limit_reached: false,
    }] },
  }] };
}
export function usageError(agent: Agent, code: UsageCode = "query_failed"): AgentUsage {
  return { agent, status: "error", entries: [], checked_at: "2026-11-14T00:00:00Z", code };
}
