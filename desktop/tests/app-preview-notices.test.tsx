import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => false }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn() }));

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { App } from "../src/context/factory/infrastructure/presentations/app";

afterEach(cleanup);

it("keeps preview guidance without deletion feedback or native events", async () => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  const attentionCard = screen.getByText("Need attention").closest("section");
  expect(attentionCard?.querySelector("strong")?.textContent).toBe("00");
  for (const page of ["Runs", "Workflows", "Settings", "Overview", "Runs"]) {
    fireEvent.click(screen.getByRole("button", { name: page }));
    expect(screen.getByText(/Interface preview\. Live data and runs/)).toBeTruthy();
    expect(screen.queryByText("Run deleted.")).toBeNull();
    expect(screen.queryByText("This run is no longer available.")).toBeNull();
    expect(screen.queryByRole("button", { name: "Dismiss notice" })).toBeNull();
  }
  const footer = screen.getByText("LOCAL FIRST").closest(".sidebar-footer");
  expect(footer).not.toBeNull();
  expect(footer!.children).toHaveLength(2);
  expect(footer!.children[1].textContent).toBe("");
  expect(screen.queryByText("v0.2.0")).toBeNull();
  expect(getVersion).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
  expect(listen).not.toHaveBeenCalled();
});
it("opens contextual usage preview without native account calls", async () => {
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  fireEvent.click(screen.getByRole("button", { name: "View Usage" }));
  await screen.findByRole("dialog", { name: "Agent usage" });
  expect((await screen.findAllByText("Open AI Factory desktop to view agent usage.")).length).toBe(2);
  expect(invoke).not.toHaveBeenCalled();
});
