import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AgentUsage, UsageCode } from "../../../../../src/context/factory/domain/types";
import { deferred } from "../../../../fixtures/notices";
import { usage, usageError } from "../../../../fixtures/usage";
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({ factoryApi: { agentUsage: vi.fn() } }));
import { factoryApi } from "../../../../../src/context/factory/infrastructure/ipc/factory-api";
import { AgentUsageDialog } from "../../../../../src/context/factory/infrastructure/presentations/agent-usage-dialog";
beforeEach(() => vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => usage(agent)));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
it("names the dialog and both sections, preserves OpenCode reported percent and optional metadata", async () => {
  const result = usage(); const metric = result.entries[0].observation.metrics[0];
  metric.resets_at = "2027-01-15T08:00:00Z";
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => agent === "codex" ? result : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  expect(screen.getByRole("dialog", { name: "Agent usage" }).getAttribute("aria-modal")).toBe("true");
  await screen.findByText("Remaining: 73%");
  expect(screen.getByRole("region", { name: "Codex" })).toBeTruthy();
  expect(screen.getByRole("region", { name: "OpenCode" })).toBeTruthy();
  expect(screen.getByText("Reported usage: 37%")).toBeTruthy();
  expect(screen.getByText("Remaining percentage is not available from this OpenCode response.")).toBeTruthy();
  expect(screen.queryByText("Remaining: 63%")).toBeNull();
  expect(screen.getByText("300 minutes")).toBeTruthy();
  const codex = screen.getByRole("heading", { name: "Fixture Codex" }).closest("li")!;
  expect(codex.querySelector("time")?.getAttribute("datetime")).toBe(metric.resets_at);
  expect(screen.getByText("fixture@example.test")).toBeTruthy();
  expect(within(screen.getByRole("region", { name: "OpenCode" })).queryByText(/Resets/)).toBeNull();
});
it("contains keyboard focus, excludes disabled Refresh and closes with Escape while pending", async () => {
  const pending = deferred<AgentUsage>(); vi.mocked(factoryApi.agentUsage).mockReturnValue(pending.promise);
  const close = vi.fn(); const user = userEvent.setup();
  render(<><button>Outside</button><AgentUsageDialog onClose={close} /></>);
  const button = screen.getByRole("button", { name: "Close usage" });
  expect(document.activeElement).toBe(button);
  expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(true);
  await user.tab(); expect(document.activeElement).toBe(button);
  await user.tab({ shift: true }); expect(document.activeElement).toBe(button);
  screen.getByRole("button", { name: "Outside" }).focus(); expect(document.activeElement).toBe(button);
  await user.keyboard("{Escape}"); expect(close).toHaveBeenCalledOnce();
  await act(async () => pending.resolve(usage()));
});
it("cycles enabled controls and restores focus after unmount with fallback for a removed opener", async () => {
  const user = userEvent.setup();
  const outside = document.createElement("button"); outside.textContent = "View Usage"; document.body.append(outside); outside.focus();
  const view = render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining: 73%");
  const close = screen.getByRole("button", { name: "Close usage" });
  await user.tab({ shift: true }); expect(document.activeElement).toBe(screen.getByRole("button", { name: "Refresh" }));
  await user.tab(); expect(document.activeElement).toBe(close);
  view.unmount(); await waitFor(() => expect(document.activeElement).toBe(outside));
  const main = document.createElement("main"); main.tabIndex = -1; document.body.append(main); outside.focus();
  const next = render(<AgentUsageDialog onClose={vi.fn()} />); outside.remove(); next.unmount();
  await waitFor(() => expect(document.activeElement).toBe(main)); main.remove();
});
it.each([
  ["not_connected", "No OpenCode Console account is signed in. Limits for other providers are not reported here."],
  ["unsupported", "This CLI version does not expose compatible quota data."],
  ["cli_missing", "The agent CLI could not be found. Check its path in Settings."],
  ["no_quota_data", "No compatible quota data was reported."],
] as const)("explains OpenCode %s without hiding valid Codex", async (code, message) => {
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => agent === "codex" ? usage() : {
    ...usageError(agent, code), status: code === "not_connected" ? "unauthenticated" : "unavailable",
  });
  render(<AgentUsageDialog onClose={vi.fn()} />);
  expect(await screen.findByText(message)).toBeTruthy(); expect(screen.getByText("Remaining: 73%")).toBeTruthy();
});
it("shows every pending/failed account without a quota and distinguishes an empty list", async () => {
  const response = usage();
  response.entries = ["pending", "failed"].map((status, i) => ({ ...response.entries[0], id: String(i), label: `${status} account`,
    observation: { status: status as "pending" | "failed", metrics: [], code: (status === "pending" ? "sign_in_pending" : "sign_in_failed") as UsageCode } }));
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => agent === "codex" ? response : usage(agent));
  const user = userEvent.setup(); render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Sign-in pending. Usage cannot be queried yet.");
  expect(screen.getByText("Sign-in failed. Usage is unavailable for this account.")).toBeTruthy();
  expect(screen.queryByText(/^Remaining:/)).toBeNull();
  response.status = "no_accounts"; response.entries = [];
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  expect(await screen.findByText("No Codex accounts configured.")).toBeTruthy();
  expect(screen.getByText("Reported usage: 37%")).toBeTruthy();
});
it("sanitizes rejected IPC and retains the previous value and timestamp on refresh failure", async () => {
  let fail = false;
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => { if (fail && agent === "codex") throw new Error("fixture-secret"); return usage(agent); });
  const user = userEvent.setup(); render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining: 73%");
  const stamp = within(screen.getByRole("region", { name: "Codex" })).getByText(/Last checked/).textContent;
  fail = true; await user.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByText("Previous data. Refresh failed.");
  expect(screen.getByText("Remaining: 73%")).toBeTruthy(); expect(screen.queryByText(/fixture-secret/)).toBeNull();
  expect(within(screen.getByRole("region", { name: "Codex" })).getByText(/Last checked/).textContent).toBe(stamp);
});
it("does not let StrictMode cleanup publish the first session into the active dialog", async () => {
  const old = deferred<AgentUsage>(); let calls = 0;
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => ++calls <= 2 ? old.promise : usage(agent));
  render(<StrictMode><AgentUsageDialog onClose={vi.fn()} /></StrictMode>);
  await screen.findByText("Remaining: 73%");
  await act(async () => old.reject(new Error("fixture-secret")));
  expect(screen.getByText("Remaining: 73%")).toBeTruthy(); expect(screen.queryByText(/Previous data|fixture-secret/)).toBeNull();
});
it("keeps valid rows on a partial failure, updates siblings and renders explicit zero reported limits", async () => {
  let response = usage(); response.entries.push({ ...structuredClone(response.entries[0]), id: "unbound", label: "Unbound Codex" });
  const open = usage("opencode"); open.entries[0].observation.metrics[0].value = 0; open.entries[0].observation.metrics[0].limit_reached = true;
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => structuredClone(agent === "codex" ? response : open));
  const user = userEvent.setup(); render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByRole("heading", { name: "Unbound Codex" });
  expect(screen.getByText("Reported usage: 0%")).toBeTruthy(); expect(screen.getByText("Limit reached")).toBeTruthy();
  response.entries[0].observation = { status: "error", code: "timed_out", metrics: [] };
  response.entries[1].observation.metrics[0].value = 42;
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByText("Previous data. Refresh failed.");
  const first = screen.getByRole("heading", { name: "Fixture Codex" }).closest("li")!;
  const second = screen.getByRole("heading", { name: "Unbound Codex" }).closest("li")!;
  expect(within(first).getByText("Remaining: 73%")).toBeTruthy();
  expect(within(second).getByText("Remaining: 42%")).toBeTruthy();
  expect(within(second).queryByText(/Previous data/)).toBeNull();
});
it("shows a retryable initial failure without substituting a quota", async () => {
  vi.mocked(factoryApi.agentUsage).mockRejectedValue(new Error("fixture-secret"));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await waitFor(() => expect(screen.getAllByRole("alert").length).toBe(2));
  expect(screen.queryByText(/Remaining:|Used:|Reported usage:|Limit reached|fixture-secret/)).toBeNull();
  expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(false);
});
