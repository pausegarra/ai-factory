import { StrictMode, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AgentUsage, UsageCode } from "../../../../../src/context/factory/domain/types";
import { deferred } from "../../../../fixtures/notices";
import { usage, usageError } from "../../../../fixtures/usage";
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({ factoryApi: { agentUsage: vi.fn() } }));
import { factoryApi } from "../../../../../src/context/factory/infrastructure/ipc/factory-api";
import { AgentUsageDialog } from "../../../../../src/context/factory/infrastructure/presentations/agent-usage-dialog";
function expectNoAnnotations(node: HTMLElement) {
  expect(within(node).queryByText(/Resets in|Reset time reached|^5 hour window$|^Weekly window$/)).toBeNull();
  expect(node.querySelector("time")).toBeNull();
}

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
  expectNoAnnotations(screen.getByRole("dialog"));
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
  expectNoAnnotations(screen.getByRole("region", { name: "OpenCode" }));
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
  expectNoAnnotations(screen.getByRole("dialog"));
  expect(screen.queryByRole("progressbar")).toBeNull();
  response.status = "no_accounts"; response.entries = [];
  await user.click(screen.getByRole("button", { name: "Refresh" }));
  expect(await screen.findByText("No Codex accounts configured.")).toBeTruthy();
  expect(screen.getByText("Remaining percentage is not available from this OpenCode response.")).toBeTruthy();
  expectNoAnnotations(screen.getByRole("dialog"));
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
  expectNoAnnotations(screen.getByRole("dialog"));
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
  expect(within(codex).getByText("5 hour window")).toBeTruthy();
  const bar = within(codex).getByRole("progressbar");
  expect(bar.getAttribute("aria-describedby")).toBeNull();
  expect(bar.getAttribute("aria-valuenow")).toBe("73");
});

it.each(["used", "reported", "limit"] as const)(
  "does not render a %s metric as remaining", async kind => {
    const response = usage();
    Object.assign(response.entries[0].observation.metrics[0], {
      kind, derivation: null, value: 37, period: "10080 minutes", resets_at: "2027-01-15T08:02:00Z",
    });
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    render(<AgentUsageDialog onClose={vi.fn()} />);
    expect(await screen.findByText(
      "Remaining percentage is not available for this account."
    )).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByText(/37%/)).toBeNull();
    expectNoAnnotations(screen.getByRole("dialog"));
  }
);

it("does not convert remaining amounts in other units into a percentage", async () => {
  const response = usage();
  Object.assign(response.entries[0].observation.metrics[0], {
    unit: "tokens", value: 2400, derivation: null, period: "10080 minutes", resets_at: "2027-01-15T08:02:00Z",
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining percentage is not available for this account.");
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText(/2400|tokens/)).toBeNull();
  expectNoAnnotations(screen.getByRole("dialog"));
});

it.each([-1, 100.1, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY])(
  "rejects invalid remaining %s without clamping it", async value => {
    const response = usage();
    Object.assign(response.entries[0].observation.metrics[0], { value, period: "10080 minutes", resets_at: "2027-01-15T08:02:00Z" });
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    render(<AgentUsageDialog onClose={vi.fn()} />);
    await screen.findByText("Remaining percentage is not available for this account.");
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByText(/^Remaining:/)).toBeNull();
    expectNoAnnotations(screen.getByRole("dialog"));
  }
);

it("rejects a corrupt reset instead of showing Invalid Date or a plausible quota", async () => {
  const response = usage();
  Object.assign(response.entries[0].observation.metrics[0], { resets_at: "bad-date", period: "10080 minutes" });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining percentage is not available for this account.");
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText(/Invalid Date|^Remaining:/)).toBeNull();
  expectNoAnnotations(screen.getByRole("dialog"));
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
  expectNoAnnotations(screen.getByRole("dialog"));
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
  response.entries[0].observation.metrics[0].resets_at = "2027-01-15T08:02:00Z";
  response.entries[0].observation.metrics.push({
    ...response.entries[0].observation.metrics[0],
    id: "corrupt", label: "Corrupt window", value: 42, resets_at: "bad-date", period: "10080 minutes",
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  const bar = await screen.findByRole("progressbar", {
    name: "Codex Fixture Codex Primary window remaining",
  });
  expect(bar.getAttribute("aria-valuenow")).toBe("73");
  expect(screen.getAllByRole("progressbar")).toHaveLength(1);
  const node = bar.closest(".usage-metric") as HTMLElement;
  expect(within(node).getByText("5 hour window")).toBeTruthy();
  expect(within(node).getByText(/Resets in|Reset time reached/)).toBeTruthy();
  expect(screen.queryByText("Weekly window")).toBeNull();
  expect(node.querySelector("time")?.getAttribute("datetime")).toBe("2027-01-15T08:02:00Z");
  expect(screen.queryByText(/Corrupt window|Invalid Date|Remaining: 42%/)).toBeNull();
});

it.each([
  ["pending", "sign_in_pending", "Sign-in pending. Usage cannot be queried yet."],
  ["failed", "sign_in_failed", "Sign-in failed. Usage is unavailable for this account."],
  ["error", "timed_out", "The usage query timed out. Try Refresh."],
  ["unauthenticated", "not_connected", "This account is not connected. Usage is unavailable."],
  ["unavailable", "no_quota_data", "No compatible quota data was reported."],
] as const)("does not show remaining from an initial %s observation", async (status, code, message) => {
  const response = usage();
  Object.assign(response.entries[0].observation.metrics[0], {
    period: "10080 minutes", resets_at: "2027-01-15T08:02:00Z",
  });
  Object.assign(response.entries[0].observation, { status, code });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "codex" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText(message);
  const codex = screen.getByRole("region", { name: "Codex" });
  expect(within(codex).queryByRole("progressbar")).toBeNull();
  expect(within(codex).queryByText(/^Remaining:/)).toBeNull();
  expect(within(codex).queryByText(/Remaining percentage is not available/)).toBeNull();
  expectNoAnnotations(codex);
});

it("keeps reported OpenCode weekly data excluded even with a valid reset", async () => {
  const response = usage("opencode");
  Object.assign(response.entries[0].observation.metrics[0], {
    id: "opencode:weekly", label: "weekly", resets_at: "2027-01-15T08:02:00Z",
  });
  vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
    agent === "opencode" ? structuredClone(response) : usage(agent));
  render(<AgentUsageDialog onClose={vi.fn()} />);
  await screen.findByText("Remaining percentage is not available from this OpenCode response.");
  const region = screen.getByRole("region", { name: "OpenCode" });
  expect(within(region).queryByRole("progressbar")).toBeNull();
  expectNoAnnotations(region);
});

function ClockHarness() {
  const [open, setOpen] = useState(true);
  return open
    ? <AgentUsageDialog onClose={() => setOpen(false)} />
    : <button onClick={() => setOpen(true)}>Open usage</button>;
}

describe("local usage clock", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    vi.setSystemTime(new Date("2027-01-15T08:00:00Z"));
  });
  afterEach(() => { cleanup(); vi.useRealTimers(); });

  it("ticks to the reset without querying the providers again", async () => {
    const response = usage();
    response.entries[0].observation.metrics[0].resets_at = "2027-01-15T08:02:00Z";
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    await act(async () => { render(<AgentUsageDialog onClose={vi.fn()} />); });
    const bar = screen.getByRole("progressbar", {
      name: "Codex Fixture Codex Primary window remaining",
    });
    const row = bar.closest(".usage-metric") as HTMLElement;
    expect(within(row).getByText("5 hour window")).toBeTruthy();
    const descriptionId = bar.getAttribute("aria-describedby");
    expect(descriptionId).toBeTruthy();
    expect(document.getElementById(descriptionId!)?.textContent).toBe("Resets in 2m");
    expect(row.querySelector("time")?.getAttribute("datetime")).toBe("2027-01-15T08:02:00Z");
    expect(row.querySelector("time")?.textContent).toBe(
      "Resets " + new Date("2027-01-15T08:02:00Z").toLocaleString()
    );
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(within(row).getByText("Resets in 1m")).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(59000); });
    expect(within(row).getByText("Resets in <1m")).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(within(row).getByText("Reset time reached")).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(within(row).getByText("Reset time reached")).toBeTruthy();
    expect(within(row).queryByText(/Resets in -/)).toBeNull();
    expect(bar.getAttribute("aria-valuenow")).toBe("73");
    expect((bar.firstElementChild as HTMLElement).style.width).toBe("73%");
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(2);
  });

  it("keeps counting the previous reset after a failed Refresh", async () => {
    const response = usage();
    response.entries[0].observation.metrics[0].resets_at = "2027-01-15T08:02:00Z";
    let fail = false;
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent => {
      if (fail && agent === "codex") throw new Error("fixture-secret");
      return agent === "codex" ? structuredClone(response) : usage(agent);
    });
    await act(async () => { render(<AgentUsageDialog onClose={vi.fn()} />); });
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    fail = true;
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Refresh" })); });
    expect(screen.getByText("Previous data. Refresh failed.")).toBeTruthy();
    const region = screen.getByRole("region", { name: "Codex" });
    expect(within(region).getByText("Resets in 1m")).toBeTruthy();
    expect(region.querySelector("time")?.getAttribute("datetime")).toBe("2027-01-15T08:02:00Z");
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(within(region).getByText("Reset time reached")).toBeTruthy();
    expect(within(region).getByText("Previous data. Refresh failed.")).toBeTruthy();
    expect(screen.queryByText(/fixture-secret/)).toBeNull();
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(4);
  });

  it("uses the new reset and window metadata after a successful Refresh", async () => {
    const response = usage();
    const metric = response.entries[0].observation.metrics[0];
    metric.resets_at = "2027-01-15T08:02:00Z";
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    await act(async () => { render(<AgentUsageDialog onClose={vi.fn()} />); });
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(screen.getByText("Resets in 1m")).toBeTruthy();
    Object.assign(metric, { period: "10080 minutes", resets_at: "2027-01-16T08:01:00Z", value: 42 });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Refresh" })); });
    const bar = screen.getByRole("progressbar", { name: "Codex Fixture Codex Primary window remaining" });
    const node = bar.closest(".usage-metric") as HTMLElement;
    expect(within(node).getByText("Weekly window")).toBeTruthy();
    expect(within(node).getByText("Resets in 1d 0h 0m")).toBeTruthy();
    expect(within(node).queryByText("5 hour window")).toBeNull();
    expect(node.querySelector("time")?.getAttribute("datetime")).toBe(metric.resets_at);
    expect(bar.getAttribute("aria-valuenow")).toBe("42");
    expect((bar.firstElementChild as HTMLElement).style.width).toBe("42%");
    expect(screen.queryByText("Previous data. Refresh failed.")).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(within(node).getByText("Resets in 23h 59m")).toBeTruthy();
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(4);
  });

  it("keeps every account and window independent, including a weekly reset due soon", async () => {
    const response = usage();
    const first = response.entries[0];
    const primary = first.observation.metrics[0];
    primary.resets_at = "2027-01-15T08:00:30Z";
    first.observation.metrics.push({
      ...primary, id: "codex:secondary", label: "Secondary window", period: "10080 minutes",
      value: 42, resets_at: "2027-01-15T08:02:00Z",
    });
    const second = structuredClone(first);
    second.id = "second-account";
    second.label = "Second Codex";
    second.observation.metrics = [{
      ...primary, period: "360 minutes", value: 61, resets_at: "2027-01-15T08:04:00Z",
    }];
    response.entries.push(second);
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    await act(async () => { render(<AgentUsageDialog onClose={vi.fn()} />); });
    const cases = [
      { account: first.label, metric: primary, label: "5 hour window", initial: "Resets in <1m", next: "Reset time reached" },
      { account: first.label, metric: first.observation.metrics[1], label: "Weekly window", initial: "Resets in 2m", next: "Resets in 1m" },
      { account: second.label, metric: second.observation.metrics[0], label: null, initial: "Resets in 4m", next: "Resets in 3m" },
    ];
    const descriptions = new Set<string>();
    const assertRows = (advanced: boolean) => {
      for (const testCase of cases) {
        const { account, metric, label } = testCase;
        const bar = screen.getByRole("progressbar", { name: `Codex ${account} ${metric.label} remaining` });
        const node = bar.closest(".usage-metric") as HTMLElement;
        expect(bar.getAttribute("aria-valuenow")).toBe(String(metric.value));
        expect((bar.firstElementChild as HTMLElement).style.width).toBe(`${metric.value}%`);
        expect(node.querySelector("time")?.getAttribute("datetime")).toBe(metric.resets_at);
        const expected = advanced ? testCase.next : testCase.initial;
        expect(within(node).getByText(expected)).toBeTruthy();
        const id = bar.getAttribute("aria-describedby");
        expect(id).toBeTruthy();
        expect(document.getElementById(id!)?.textContent).toBe(expected);
        if (!advanced) descriptions.add(id!);
        if (label === null) expect(within(node).queryByText(/^(5 hour window|Weekly window)$/)).toBeNull();
        else expect(within(node).getByText(label)).toBeTruthy();
      }
    };
    assertRows(false);
    expect(descriptions.size).toBe(3);
    await act(async () => { await vi.advanceTimersByTimeAsync(30000); });
    assertRows(true);
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(2);
  });

  it.each([
    ["opencode:weekly", "weekly", "Weekly window"],
    ["opencode:rolling", "rolling", null],
    ["opencode:monthly", "monthly", null],
    ["opencode:unknown", "custom", null],
  ] as const)("annotates explicit remaining OpenCode window %s conservatively", async (id, label, expected) => {
    const open = usage("opencode");
    Object.assign(open.entries[0].observation.metrics[0], {
      id, label, kind: "remaining", value: 64.5, period: null, resets_at: "2027-01-15T08:02:00Z",
    });
    const codex = usage(); codex.status = "no_accounts"; codex.entries = [];
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      structuredClone(agent === "opencode" ? open : codex));
    await act(async () => { render(<AgentUsageDialog onClose={vi.fn()} />); });
    const bar = screen.getByRole("progressbar", { name: `OpenCode OpenCode Console ${label} remaining` });
    const metricNode = bar.closest(".usage-metric") as HTMLElement;
    expect(within(metricNode).getByText("Remaining: 64.5%")).toBeTruthy();
    expect(within(metricNode).getByText("Resets in 2m")).toBeTruthy();
    expect(metricNode.querySelector("time")?.getAttribute("datetime")).toBe("2027-01-15T08:02:00Z");
    if (expected === null) expect(within(metricNode).queryByText(/^(5 hour window|Weekly window)$/)).toBeNull();
    else expect(within(metricNode).getByText(expected)).toBeTruthy();
  });

  it.each(["button", "escape"] as const)("stops the clock on %s close and starts fresh on reopen", async method => {
    const response = usage();
    response.entries[0].observation.metrics[0].resets_at = "2027-01-15T08:02:00Z";
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    const baseline = vi.getTimerCount();
    await act(async () => { render(<ClockHarness />); });
    expect(vi.getTimerCount()).toBe(baseline + 1);
    expect(screen.getByText("Resets in 2m")).toBeTruthy();
    await act(async () => {
      if (method === "button") fireEvent.click(screen.getByRole("button", { name: "Close usage" }));
      else fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(vi.getTimerCount()).toBe(baseline);
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(2);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Open usage" })); });
    expect(screen.getByText("Resets in 1m")).toBeTruthy();
    expect(vi.getTimerCount()).toBe(baseline + 1);
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(4);
  });

  it("keeps exactly one clock through StrictMode effect replay and clears it on unmount", async () => {
    const baseline = vi.getTimerCount();
    let mounted!: ReturnType<typeof render>;
    await act(async () => { mounted = render(<StrictMode><AgentUsageDialog onClose={vi.fn()} /></StrictMode>); });
    expect(vi.getTimerCount()).toBe(baseline + 1);
    const calls = vi.mocked(factoryApi.agentUsage).mock.calls.length;
    expect(calls).toBe(4);
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(calls);
    mounted.unmount();
    expect(vi.getTimerCount()).toBe(baseline);
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(calls);
  });

  it("recomputes from the current instant after a delayed tick", async () => {
    const response = usage();
    response.entries[0].observation.metrics[0].resets_at = "2027-01-15T10:15:00Z";
    vi.mocked(factoryApi.agentUsage).mockImplementation(async agent =>
      agent === "codex" ? structuredClone(response) : usage(agent));
    await act(async () => { render(<AgentUsageDialog onClose={vi.fn()} />); });
    expect(screen.getByText("Resets in 2h 15m")).toBeTruthy();
    vi.setSystemTime(new Date("2027-01-15T10:14:30Z"));
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(screen.getByText("Resets in <1m")).toBeTruthy();
    expect(factoryApi.agentUsage).toHaveBeenCalledTimes(2);
  });
});
