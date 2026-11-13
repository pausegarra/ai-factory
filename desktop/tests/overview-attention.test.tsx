import { afterEach, expect, it } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import { OverviewPage } from "../src/context/factory/infrastructure/presentations/overview";
import { attentionHistory as history, attentionRun as run,
  attentionSettings as settings, attentionSnapshot as snapshot } from "./fixtures/attention";
afterEach(cleanup);
function metric(label: string): string | null | undefined {
  const labelElement = screen.getAllByText(label).find(element => element.closest("section.metric"));
  return labelElement?.closest("section.metric")?.querySelector("strong")?.textContent;
}

it("resolves attention while keeping historical metrics and failed rows", () => {
  const [failed, success] = history(["failed", "succeeded"]);
  failed.error = "original failure";
  const value = snapshot([failed, success, run("other-running", "running", { event_id: "other" })]);
  const before = structuredClone(value);
  const view = render(<OverviewPage snapshot={value} settings={settings} onPage={() => {}} onRun={() => {}} />);
  expect(metric("Need attention")).toBe("00");
  expect(metric("Running")).toBe("01");
  expect(metric("Queued")).toBe("00");
  expect(metric("Completed")).toBe("01");
  expect(screen.getByText("3 total tasks")).toBeTruthy();
  expect(screen.getByText("3 slots available for new tasks.")).toBeTruthy();
  expect(value.metrics).toEqual({ total: 3, failed: 1, succeeded: 1, running: 1,
    queued: 0, cancelled: 0, interrupted: 0 });
  // Active runs occupy recent activity while running. Remove that run to inspect history.
  view.rerender(<OverviewPage snapshot={snapshot([failed, success])} settings={settings} onPage={() => {}} onRun={() => {}} />);
  const row = screen.getByRole("button", { name: failed.title }).closest("tr")!;
  expect(within(row).getByText("Failed")).toBeTruthy();
  expect(screen.getByRole("button", { name: success.title })).toBeTruthy();
  expect(metric("Need attention")).toBe("00");
  expect(value).toEqual(before);
});

it("resolves blocked approvals after success on a new SHA while preserving history", () => {
  const runs = history(["blocked", "blocked", "succeeded"]).map((item, index) => ({
    ...item, trigger_id: "pr_approved" as const,
    event_id: `approval:owner/repo#26@${index < 2 ? "old-sha" : "new-sha"}`,
    attempt: index === 1 ? 3 : 1,
  }));
  const value = snapshot(runs);
  const before = structuredClone(value);
  render(<OverviewPage snapshot={value} settings={settings} onPage={() => {}} onRun={() => {}} />);
  expect(metric("Need attention")).toBe("00");
  expect(metric("Completed")).toBe("01");
  expect(screen.getAllByText("Blocked")).toHaveLength(2);
  expect(screen.getByText("3 total tasks")).toBeTruthy();
  expect(value).toEqual(before);
});

it("uses all visible history beyond the five recent rows", () => {
  const recent = Array.from({ length: 5 }, (_, i) => run(`recent-${i}`, "succeeded", { event_id: `recent-${i}` }));
  const resolved = history(["failed", "succeeded"], "resolved-event");
  const pending = run("pending-outside-five", "failed", { event_id: "pending-event" });
  render(<OverviewPage snapshot={snapshot([...recent, ...resolved, pending])}
    settings={settings} onPage={() => {}} onRun={() => {}} />);
  expect(metric("Need attention")).toBe("01");
  expect(screen.queryByRole("button", { name: pending.title })).toBeNull();
  expect(screen.queryByRole("button", { name: resolved[0].title })).toBeNull();
  expect(screen.getAllByRole("row")).toHaveLength(6);
});

it("renders unknown statuses conservatively and keeps two digit formatting", () => {
  const runs = history(["future_status"]);
  const view = render(<OverviewPage snapshot={snapshot(runs)} settings={settings} onPage={() => {}} onRun={() => {}} />);
  expect(metric("Need attention")).toBe("01");
  view.rerender(<OverviewPage snapshot={snapshot(history(["future_status", "succeeded"]))}
    settings={settings} onPage={() => {}} onRun={() => {}} />);
  expect(metric("Need attention")).toBe("00");
});
