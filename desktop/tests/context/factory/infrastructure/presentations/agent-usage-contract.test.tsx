import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import contract from "../../../../../../crates/factory-core/tests/fixtures/agent_usage/codex-remaining-contract.json";
import type { AgentUsage } from "../../../../../src/context/factory/domain/types";
import { AgentUsageDialog } from "../../../../../src/context/factory/infrastructure/presentations/agent-usage-dialog";
afterEach(cleanup);
it.each(contract.cases)("shows parser-backed remaining contract: $name", async testCase => {
  vi.mocked(invoke).mockImplementation(async <T,>(command: string, args?: Parameters<typeof invoke>[1]): Promise<T> => {
    expect(command).toBe("get_agent_usage");
    if ((args as { agent: string }).agent === "codex") return structuredClone(testCase.wire) as AgentUsage as T;
    return { agent: "opencode", status: "unavailable", entries: [], checked_at: "2026-11-13T00:00:00Z", code: "unsupported" } satisfies AgentUsage as T;
  });
  render(<AgentUsageDialog onClose={vi.fn()} />);
  const heading = await screen.findByRole("heading", { name: "Fixture Codex" });
  const row = heading.closest("li") as HTMLElement;
  expect(row).not.toBeNull();
  if (testCase.remaining !== null) {
    expect(within(row).getByText(`Remaining: ${testCase.remaining}%`)).toBeTruthy();
    const bar = within(row).getByRole("progressbar", {
      name: "Codex Fixture Codex Primary window remaining",
    });
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    expect(bar.getAttribute("aria-valuenow")).toBe(String(testCase.remaining));
    expect((bar.firstElementChild as HTMLElement).style.width).toBe(testCase.remaining + "%");
    expect(within(row).queryByText(/Remaining percentage is not available/)).toBeNull();
  } else {
    expect(within(row).queryByText(/Remaining:|Used:|Reported usage:/)).toBeNull();
    expect(within(row).queryByRole("progressbar")).toBeNull();
  }
  expect(within(row).queryByText("Calculated as 100% minus Codex reported usage.")).toBeNull();
  expect(within(row).queryByTitle("Source: Codex usedPercent")).toBeNull();
  expect(within(row).queryByText(/Last checked|300 minutes|fixture@example.test/)).toBeNull();
  expect(invoke).toHaveBeenCalledWith("get_agent_usage", { agent: "opencode" });
  expect(within(row).queryByText(/^Used:/)).toBeNull();
  expect(within(row).queryByText(/tokens/i)).toBeNull();
  expect(screen.queryByText(/fixture-secret/)).toBeNull();
  expect(invoke).toHaveBeenCalledWith("get_agent_usage", { agent: "codex" });
});
