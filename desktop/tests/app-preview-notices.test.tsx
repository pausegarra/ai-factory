import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => false }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

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
  for (const page of ["Runs", "Workflows", "Settings", "Overview", "Runs"]) {
    fireEvent.click(screen.getByRole("button", { name: page }));
    expect(screen.getByText(/Interface preview\. Live data and runs/)).toBeTruthy();
    expect(screen.queryByText("Run deleted.")).toBeNull();
    expect(screen.queryByText("This run is no longer available.")).toBeNull();
    expect(screen.queryByRole("button", { name: "Dismiss notice" })).toBeNull();
  }
  expect(invoke).not.toHaveBeenCalled();
  expect(listen).not.toHaveBeenCalled();
});
