import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { attentionRun, attentionSnapshot } from "./fixtures/attention";
import { makeDetail } from "./fixtures/run-detail";
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    detail: vi.fn(), logs: vi.fn(async () => ({ text: "", offset: 0, next_offset: 0, eof: true })),
    subscribe: vi.fn(async () => () => {}),
  },
}));
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { RunsPage, RunDetailPage } from "../src/context/factory/infrastructure/presentations/runs";
afterEach(cleanup);

it("filters Release history and opens the selected run", () => {
  const release = attentionRun("release-run", "queued", { trigger_id: "release", title: "[Release] 2.0.0" });
  const issue = attentionRun("issue-run", "queued");
  const select = vi.fn();
  render(<RunsPage snapshot={attentionSnapshot([release, issue])} onSelect={select} />);
  fireEvent.change(screen.getByLabelText("Filter trigger"), { target: { value: "release" } });
  expect(screen.queryByText(issue.title)).toBeNull();
  fireEvent.click(screen.getByText(release.title));
  expect(select).toHaveBeenCalledExactlyOnceWith("release-run");
  fireEvent.change(screen.getByLabelText("Filter trigger"), { target: { value: "all" } });
  expect(screen.getByText(issue.title)).toBeTruthy();
  expect(screen.getByText(release.title)).toBeTruthy();
});

it("labels Release in the persisted run detail", async () => {
  const detail = makeDetail([["publish_app", []]]);
  detail.run.trigger_id = "release";
  detail.flow.id = "release";
  detail.run.title = "[Release] detail";
  vi.mocked(factoryApi.detail).mockResolvedValue({ state: "available", detail });
  render(<RunDetailPage id={detail.run.id} onBack={vi.fn()} onSelect={vi.fn()} />);
  await screen.findByRole("heading", { name: "[Release] detail" });
  expect(screen.getByText(/Release · Attempt 1/)).toBeTruthy();
});
