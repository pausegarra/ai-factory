import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, it, expect } from "vitest";
import { OverviewPage } from "../src/context/factory/infrastructure/presentations/overview";
import { attentionSettings, attentionSnapshot, attentionHistory } from "./fixtures/attention";
afterEach(cleanup);
it("uses reserved capacity independently of historical run states", () => {
  const value = attentionSnapshot(attentionHistory(["queued", "queued", "queued"]));
  value.capacity = { global: { active: 3, limit: 8 }, by_trigger: {
    new_issue: { active: 1, limit: 4 }, pr_review: { active: 0, limit: 3 }, pr_approved: { active: 2, limit: 2 },
  } };
  render(<OverviewPage snapshot={value} settings={attentionSettings} onPage={() => {}} onRun={() => {}} />);
  const panel = screen.getByRole("region", { name: "Capacity" });
  expect(within(panel).getByLabelText("Global capacity").textContent).toBe("3 / 8");
  expect(within(panel).getByLabelText("New issue capacity").textContent).toBe("1 / 4");
  expect(within(panel).getByLabelText("PR review capacity").textContent).toBe("0 / 3");
  expect(within(panel).getByLabelText("PR approved capacity").textContent).toBe("2 / 2");
  expect(within(panel).getByText("5 slots available for new tasks.")).toBeTruthy();
});
it("shows real occupancy above a reduced limit until workers drain", () => {
  const value = attentionSnapshot([]);
  value.capacity = { global: { active: 2, limit: 1 }, by_trigger: {
    new_issue: { active: 2, limit: 1 }, pr_review: { active: 0, limit: 1 }, pr_approved: { active: 0, limit: 1 },
  } };
  const props = { settings: attentionSettings, onPage: () => {}, onRun: () => {} };
  const view = render(<OverviewPage snapshot={value} {...props} />);
  expect(screen.getByLabelText("Global capacity").textContent).toBe("2 / 1");
  expect(screen.getByLabelText("New issue capacity").textContent).toBe("2 / 1");
  expect(screen.getByText("0 slots available for new tasks.")).toBeTruthy();
  view.rerender(<OverviewPage snapshot={attentionSnapshot(attentionHistory(["queued"]))} {...props} />);
  expect(screen.getByLabelText("Global capacity").textContent).toBe("0 / 4");
});
