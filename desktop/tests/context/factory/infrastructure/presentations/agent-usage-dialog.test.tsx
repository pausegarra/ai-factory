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
it("shows an accessible remaining summary and only its own reset", async () => {
  const response = usage();
  const metric = response.entries[0].observation.metrics[0];
  metric.resets_at = "2027-01-15T08:00:00Z";
  metric.limit_reached = true;
  response.entries[0].observation.metrics.push({
    ...metric, id: "used", label: "Hidden usage", kind: "used",
    derivation: null, value: 27,
  }, {
    ...metric, id: "limit", label: "Hidden limit", kind: "limit",
    derivation: null, value: 100,
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  const bar = await screen.findByRole("progressbar", {
    name: "Codex Fixture Codex Primary window remaining",
  });
  expect(bar.getAttribute("aria-valuemin")).toBe("0");
  expect(bar.getAttribute("aria-valuemax")).toBe("100");
  expect(bar.getAttribute("aria-valuenow")).toBe("73");
  expect((bar.firstElementChild as HTMLElement).style.width).toBe("73%");
  expect(screen.getByText("Remaining: 73%")).toBeTruthy();
  expect(screen.getByRole("dialog", { name: "Agent usage" })
    .getAttribute("aria-modal")).toBe("true");
  const row = screen.getByRole("heading", { name: "Fixture Codex" }).closest("li")!;
  const time = row.querySelector("time")!;
  expect(time.getAttribute("datetime")).toBe(metric.resets_at);
  expect(time.textContent).toBe("Resets " + new Date(metric.resets_at).toLocaleString());
  expect(within(row).queryByText("fixture@example.test")).toBeNull();
  expect(within(row).queryByText("300 minutes")).toBeNull();
  expect(within(row).queryByText(/Last checked|Calculated as|Limit reached/)).toBeNull();
  expect(within(row).queryByTitle("Source: Codex usedPercent")).toBeNull();
  expect(within(row).queryByText(/Hidden usage|Hidden limit|^Used:|^Limit:/)).toBeNull();
  const open = screen.getByRole("region", { name: "OpenCode" });
  expect(within(open).queryByRole("progressbar")).toBeNull();
  expect(within(open).queryByText(/37%|^Reported usage:|^Remaining:/)).toBeNull();
  expect(within(open).getByText(
    "Remaining percentage is not available from this OpenCode response."
  )).toBeTruthy();
  expect(open.querySelector("time")).toBeNull();
});
it("contains keyboard focus, excludes disabled Refresh and closes with Escape while pending", async () => {
  const pending = deferred<AgentUsage>(); vi.mocked(factoryApi.agentUsage).mockReturnValue(pending.promise);
  const close = vi.fn(); const user = userEvent.setup();
  render(<><button>Outside</button><AgentUsageDialog onClose={close} /></>);
  expect(screen.getAllByText("Checking usage…")).toHaveLength(2);
  expect(screen.queryByRole("progressbar")).toBeNull();
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
  expect(screen.queryByRole("progressbar")).toBeNull();
  response.status = "no_accounts"; response.entries = [];
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  expect(await screen.findByText("No Codex accounts configured.")).toBeTruthy();
  expect(screen.getByText("Remaining percentage is not available from this OpenCode response.")).toBeTruthy();
  expect(screen.queryByRole("progressbar")).toBeNull();
});
it("retains the remaining bar and reset after refresh failure without exposing raw errors", async () => {
  const response = usage();
  const reset = "2027-01-15T08:00:00Z";
  response.entries[0].observation.metrics[0].resets_at = reset;
  let fail = false;
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => {
    if (fail && agent === "codex") throw new Error("fixture-secret");
    return agent === "codex" ? structuredClone(response) : usage(agent);
  });
  const user = userEvent.setup();
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByRole("progressbar");
  fail = true;
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByText("Previous data. Refresh failed.");
  const codex = screen.getByRole("region", { name: "Codex" });
  const bar = within(codex).getByRole("progressbar");
  expect(bar.getAttribute("aria-valuenow")).toBe("73");
  expect(within(codex).getByText("Remaining: 73%")).toBeTruthy();
  expect(codex.querySelector("time")?.getAttribute("datetime")).toBe(reset);
  expect(screen.queryByText(/Last checked|fixture-secret/)).toBeNull();
  expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(false);
});
it("does not let StrictMode cleanup publish the first session into the active dialog", async () => {
  const old = deferred<AgentUsage>(); let calls = 0;
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => ++calls <= 2 ? old.promise : usage(agent));
  render(<StrictMode><AgentUsageDialog onClose={vi.fn()} /></StrictMode>);
  await screen.findByText("Remaining: 73%");
  await act(async () => old.reject(new Error("fixture-secret")));
  expect(screen.getByText("Remaining: 73%")).toBeTruthy(); expect(screen.queryByText(/Previous data|fixture-secret/)).toBeNull();
  expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("73");
});
it("keeps valid rows on a partial failure, updates siblings and does not reinterpret zero reported limits", async () => {
  const response = usage();
  response.entries[0].observation.metrics[0].resets_at = "2027-01-15T08:00:00Z";
  response.entries.push({ ...structuredClone(response.entries[0]), id: "unbound", label: "Unbound Codex" });
  response.entries[1].observation.metrics[0].resets_at = "2027-01-17T10:30:00Z";
  const open = usage("opencode"); open.entries[0].observation.metrics[0].value = 0; open.entries[0].observation.metrics[0].limit_reached = true;
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => structuredClone(agent === "codex" ? response : open));
  const user = userEvent.setup(); render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByRole("heading", { name: "Unbound Codex" });
  expect(screen.getByText("Remaining percentage is not available from this OpenCode response.")).toBeTruthy();
  expect(within(screen.getByRole("region", { name: "OpenCode" })).queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText(/Reported usage:|Limit reached/)).toBeNull();
  response.entries[0].observation = { status: "error", code: "timed_out", metrics: [] };
  response.entries[1].observation.metrics[0].value = 42;
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByText("Previous data. Refresh failed.");
  const first = screen.getByRole("heading", { name: "Fixture Codex" }).closest("li")!;
  const second = screen.getByRole("heading", { name: "Unbound Codex" }).closest("li")!;
  expect(within(first).getByText("Remaining: 73%")).toBeTruthy();
  expect(within(second).getByText("Remaining: 42%")).toBeTruthy();
  expect(within(first).getByRole("progressbar").getAttribute("aria-valuenow")).toBe("73");
  expect(first.querySelector("time")?.getAttribute("datetime")).toBe("2027-01-15T08:00:00Z");
  expect(within(second).getByRole("progressbar").getAttribute("aria-valuenow")).toBe("42");
  expect(second.querySelector("time")?.getAttribute("datetime")).toBe("2027-01-17T10:30:00Z");
  expect(within(first).getByText("Previous data. Refresh failed.")).toBeTruthy();
  expect(within(second).queryByText(/Previous data/)).toBeNull();
});
it("shows a retryable initial failure without substituting a quota", async () => {
  vi.mocked(factoryApi.agentUsage).mockRejectedValue(new Error("fixture-secret"));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await waitFor(() => expect(screen.getAllByRole("alert").length).toBe(2));
  expect(screen.queryByText(/Remaining:|Used:|Reported usage:|Limit reached|fixture-secret/)).toBeNull();
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(false);
});

it("keeps separate quota labels, values and local reset times", async () => {
  const response = usage();
  const first = response.entries[0].observation.metrics[0];
  first.resets_at = "2027-01-15T08:00:00Z";
  response.entries[0].observation.metrics.push({
    ...first, id: "codex:secondary", label: "Secondary window",
    value: 42, resets_at: "2027-01-17T10:30:00+02:00",
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining: 73%");
  for (const metric of response.entries[0].observation.metrics) {
    const bar = screen.getByRole("progressbar", {
      name: "Codex Fixture Codex " + metric.label + " remaining",
    });
    expect(bar.getAttribute("aria-valuenow")).toBe(String(metric.value));
    const summary = bar.closest(".usage-metric")!;
    expect(within(summary as HTMLElement).getByText(metric.label)).toBeTruthy();
    const time = summary.querySelector("time")!;
    expect(time.getAttribute("datetime")).toBe(metric.resets_at);
    expect(time.textContent).toBe("Resets " + new Date(metric.resets_at!).toLocaleString());
  }
  expect(screen.getAllByRole("progressbar")).toHaveLength(2);
});

it("omits a missing reset instead of deriving it from the period", async () => {
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByRole("progressbar");
  const codex = screen.getByRole("region", { name: "Codex" });
  expect(codex.querySelector("time")).toBeNull();
  expect(within(codex).queryByText(/Resets|300 minutes/)).toBeNull();
});

it.each(["used", "reported", "limit"] as const)(
  "does not render a %s metric as remaining", async kind => {
    const response = usage();
    Object.assign(response.entries[0].observation.metrics[0], {
      kind, derivation: null, value: 37,
    });
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    render(<AgentUsageDialog onClose={vi.fn()} />);
    expect(await screen.findByText(
      "Remaining percentage is not available for this account."
    )).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByText(/37%/)).toBeNull();
  }
);

it("does not convert remaining amounts in other units into a percentage", async () => {
  const response = usage();
  Object.assign(response.entries[0].observation.metrics[0], {
    unit: "tokens", value: 2400, derivation: null,
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining percentage is not available for this account.");
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText(/2400|tokens/)).toBeNull();
});

it.each([-1, 100.1, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY])(
  "rejects invalid remaining %s without clamping it", async value => {
    const response = usage();
    response.entries[0].observation.metrics[0].value = value;
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    render(<AgentUsageDialog onClose={vi.fn()} />);
    await screen.findByText("Remaining percentage is not available for this account.");
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByText(/^Remaining:/)).toBeNull();
  }
);

it("rejects a corrupt reset instead of showing Invalid Date or a plausible quota", async () => {
  const response = usage();
  response.entries[0].observation.metrics[0].resets_at = "bad-date";
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining percentage is not available for this account.");
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText(/Invalid Date|^Remaining:/)).toBeNull();
});

it("shows explicit remaining for OpenCode even when reported metrics coexist", async () => {
  const response = usage("opencode");
  response.entries[0].observation.metrics.push({
    id: "open:remaining", label: "Explicit window", kind: "remaining",
    derivation: null, value: 64.5, unit: "percent", period: null,
    resets_at: null, limit_reached: null,
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "opencode" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  const bar = await screen.findByRole("progressbar", {
    name: "OpenCode OpenCode Console Explicit window remaining",
  });
  expect(bar.getAttribute("aria-valuenow")).toBe("64.5");
  expect(screen.getByText("Remaining: 64.5%")).toBeTruthy();
  expect(screen.queryByText(/37%|Remaining percentage is not available/)).toBeNull();
});

it("explains an available account with no compatible quota", async () => {
  const response = usage();
  response.entries[0].observation.metrics = [];
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining percentage is not available for this account.");
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText(/^Remaining:/)).toBeNull();
});

it("keeps Refresh disabled until both refreshed sources settle", async () => {
  const codex = deferred<AgentUsage>();
  const open = deferred<AgentUsage>();
  const user = userEvent.setup();
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByRole("progressbar");
  await waitFor(() => expect(
    (screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled
  ).toBe(false));
  expect(factoryApi.agentUsage).toHaveBeenCalledTimes(2);
  vi.mocked(factoryApi.agentUsage).mockImplementation(agent =>
    agent === "codex" ? codex.promise : open.promise);
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  const refresh = screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement;
  expect(refresh.disabled).toBe(true);
  expect(factoryApi.agentUsage).toHaveBeenCalledTimes(4);
  await user.click(refresh);
  expect(factoryApi.agentUsage).toHaveBeenCalledTimes(4);
  await act(async () => codex.resolve(usage()));
  expect(refresh.disabled).toBe(true);
  await act(async () => open.resolve(usage("opencode")));
  await waitFor(() => expect(refresh.disabled).toBe(false));
  expect(factoryApi.agentUsage).toHaveBeenCalledWith("codex");
  expect(factoryApi.agentUsage).toHaveBeenCalledWith("opencode");
});

it("keeps a valid quota when a sibling has a corrupt reset", async () => {
  const response = usage();
  response.entries[0].observation.metrics.push({
    ...response.entries[0].observation.metrics[0],
    id: "corrupt", label: "Corrupt window", value: 42, resets_at: "bad-date",
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  const bar = await screen.findByRole("progressbar", {
    name: "Codex Fixture Codex Primary window remaining",
  });
  expect(bar.getAttribute("aria-valuenow")).toBe("73");
  expect(screen.getAllByRole("progressbar")).toHaveLength(1);
  expect(screen.queryByText(/Corrupt window|Invalid Date|Remaining: 42%/)).toBeNull();
});

it.each([
  ["pending", "sign_in_pending", "Sign-in pending. Usage cannot be queried yet."],
  ["failed", "sign_in_failed", "Sign-in failed. Usage is unavailable for this account."],
  ["error", "timed_out", "The usage query timed out. Try Refresh."],
] as const)("does not show remaining from an initial %s observation", async (status, code, message) => {
  const response = usage();
  Object.assign(response.entries[0].observation, { status, code });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText(message);
  const codex = screen.getByRole("region", { name: "Codex" });
  expect(within(codex).queryByRole("progressbar")).toBeNull();
  expect(within(codex).queryByText(/^Remaining:/)).toBeNull();
  expect(within(codex).queryByText(/Remaining percentage is not available/)).toBeNull();
});
