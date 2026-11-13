import { afterEach, expect, it, vi } from "vitest";
import { AgentUsageSession, type UsageView } from "../../../../src/context/factory/application/agent-usage";
import { deferred } from "../../../fixtures/notices";
import { usage, usageError } from "../../../fixtures/usage";
import type { AgentUsage } from "../../../../src/context/factory/domain/types";
afterEach(() => vi.useRealTimers());
it("starts both agents, publishes each independently and rejects duplicate refresh", async () => {
  const codex = deferred<AgentUsage>(), opencode = deferred<AgentUsage>();
  let view!: UsageView;
  const load = vi.fn(agent => agent === "codex" ? codex.promise : opencode.promise);
  const session = new AgentUsageSession(load, value => { view = value; });
  expect(load).not.toHaveBeenCalled();
  const pending = session.refresh();
  await session.refresh();
  expect(view.codex.loading && view.opencode.loading).toBe(true);
  opencode.resolve(usage("opencode")); await Promise.resolve(); await Promise.resolve();
  expect(view.opencode.entries[0].entry.observation.metrics[0].value).toBe(37);
  expect(view.opencode.loading).toBe(false); expect(view.codex.loading).toBe(true);
  expect(load).toHaveBeenCalledTimes(2);
  codex.resolve(usage()); await pending;
  expect(view.codex.entries[0].entry.observation.metrics[0].value).toBe(73);
  session.close();
});
it("keeps only previously valid rows stale with their original timestamps on partial refresh failure", async () => {
  let current = usage();
  current.entries.push({ ...structuredClone(current.entries[0]), id: "other", label: "Unbound" });
  let view!: UsageView;
  const session = new AgentUsageSession(async agent => agent === "codex" ? structuredClone(current) : usage("opencode"), next => { view = next; });
  await session.refresh();
  const original = view.codex.entries[0].entry.checked_at;
  current.entries[0].observation = { status: "error", code: "timed_out", metrics: [] };
  current.entries[0].checked_at = "2026-11-14T00:00:00Z";
  current.entries[1].observation.metrics[0].value = 42;
  await session.refresh();
  expect(view.codex.entries[0].stale).toBe(true);
  expect(view.codex.entries[0].error).toBe("timed_out");
  expect(view.codex.entries[0].entry.checked_at).toBe(original);
  expect(view.codex.entries[0].entry.observation.metrics[0].value).toBe(73);
  expect(view.codex.entries[1].stale).toBe(false);
  expect(view.codex.entries[1].entry.observation.metrics[0].value).toBe(42);
  current.entries.shift(); await session.refresh();
  expect(view.codex.entries.map(row => row.entry.id)).toEqual(["other"]);
  session.close();
});
it("keeps old data on rejected or section failures but clears it for authoritative absence and sign-in states", async () => {
  let response: AgentUsage | Error = usage(); let view!: UsageView;
  const session = new AgentUsageSession(async agent => {
    if (agent === "opencode") return usageError(agent, "unsupported");
    if (response instanceof Error) throw response; return response;
  }, next => { view = next; });
  await session.refresh(); response = new Error("fixture-secret"); await session.refresh();
  expect(view.codex.entries[0].stale).toBe(true); expect(view.codex.error).toBe("query_failed");
  expect(JSON.stringify(view)).not.toContain("fixture-secret");
  response = usageError("codex", "cli_missing"); await session.refresh();
  expect(view.codex.entries[0].stale).toBe(true); expect(view.codex.error).toBe("cli_missing");
  response = usage(); response.entries[0].observation = { status: "pending", code: "sign_in_pending", metrics: [] };
  await session.refresh(); expect(view.codex.entries[0].stale).toBe(false); expect(view.codex.entries[0].entry.observation.metrics).toEqual([]);
  response = { ...usage(), status: "no_accounts", entries: [] }; await session.refresh(); expect(view.codex.entries).toEqual([]);
  session.close();
});
it.each([true, false])("ignores closed-session completion and rejection across reopening, old-first=%s", async oldFirst => {
  const old = deferred<AgentUsage>(), fresh = deferred<AgentUsage>();
  const changed = vi.fn();
  const first = new AgentUsageSession(() => old.promise, changed);
  const oldPending = first.refresh(); first.close(); const count = changed.mock.calls.length;
  let view!: UsageView;
  const next = new AgentUsageSession(async agent => agent === "codex" ? fresh.promise : usage("opencode"), value => { view = value; });
  const newPending = next.refresh();
  if (oldFirst) { old.reject(new Error("fixture-secret")); await oldPending; }
  fresh.resolve(usage()); await newPending;
  if (!oldFirst) { old.reject(new Error("fixture-secret")); await oldPending; }
  expect(view.codex.entries[0].entry.observation.metrics[0].value).toBe(73);
  expect(changed).toHaveBeenCalledTimes(count);
  await first.refresh(); expect(changed).toHaveBeenCalledTimes(count); next.close();
});
it("does not poll or carry a closed session cache", async () => {
  vi.useFakeTimers(); const load = vi.fn(async agent => usage(agent)); let view!: UsageView;
  const session = new AgentUsageSession(load, next => { view = next; });
  await session.refresh(); await vi.advanceTimersByTimeAsync(60000);
  expect(load).toHaveBeenCalledTimes(2); session.close();
  const next = new AgentUsageSession(async agent => usageError(agent), value => { view = value; });
  await next.refresh(); expect(view.codex.entries).toEqual([]); next.close();
});
it.each(["pending", "failed", "unauthenticated", "unavailable"] as const)("replaces prior quota for authoritative %s", async status => {
  let current = usage(); let view!: UsageView;
  const session = new AgentUsageSession(async agent => agent === "codex" ? structuredClone(current) : usage(agent), next => { view = next; });
  await session.refresh();
  current.entries[0].observation = { status, code: "not_connected", metrics: [] };
  await session.refresh();
  expect(view.codex.entries[0].stale).toBe(false);
  expect(view.codex.entries[0].entry.observation.metrics).toEqual([]);
  session.close();
});
it("does not inherit metrics by label or mutate previously published snapshots", async () => {
  let response = usage(); let view!: UsageView;
  const session = new AgentUsageSession(async agent => agent === "codex" ? structuredClone(response) : usage(agent), next => { view = next; });
  await session.refresh(); const previous = view;
  response.entries[0].id = "new-id-same-label";
  response.entries[0].observation = { status: "error", code: "protocol", metrics: [] };
  await session.refresh();
  expect(view.codex.entries[0].stale).toBe(false); expect(view.codex.entries[0].entry.observation.metrics).toEqual([]);
  expect(previous.codex.entries[0].entry.observation.metrics[0].value).toBe(73);
  expect(previous.codex.loading).toBe(false); session.close();
});
