import { afterEach, beforeEach, it, expect, vi } from "vitest";
import {
  render,
  screen,
  cleanup,
  fireEvent,
  waitFor,
  within,
} from "@testing-library/react";
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    importLegacy: vi
      .fn()
      .mockResolvedValue({
        runs_imported: 7,
        settings_imported: false,
        archive_path: "/app/archive",
        warnings: [],
      }),
  },
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { open } from "@tauri-apps/plugin-dialog";
import { LegacyImport } from "../src/context/factory/infrastructure/presentations/legacy-import";
afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(factoryApi.importLegacy).mockResolvedValue({
    runs_imported: 7,
    settings_imported: false,
    archive_path: "/app/archive",
    warnings: [],
  });
  vi.mocked(open).mockReset();
});
it("imports explicit files and shows archive evidence", async () => {
  const imported = vi.fn();
  render(<LegacyImport disabled={false} onImported={imported} />);
  fireEvent.change(screen.getByLabelText("Previous SQLite database"), {
    target: { value: "/old/state.db" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import data" }));
  await waitFor(() =>
    expect(factoryApi.importLegacy).toHaveBeenCalledWith(null, "/old/state.db"),
  );
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toContain("7 runs"),
  );
  expect(screen.getByText("/app/archive")).toBeTruthy();
  expect(imported).toHaveBeenCalled();
});
it("requires resolving unsaved settings before import", () => {
  render(<LegacyImport disabled={true} onImported={vi.fn()} />);
  const button = screen.getByRole("button", {
    name: "Import data",
  }) as HTMLButtonElement;
  expect(button.disabled).toBe(true);
  expect(screen.getByText(/Save or discard/)).toBeTruthy();
});

it("keeps confirmed import evidence when refreshing settings fails", async () => {
  const imported = vi.fn().mockRejectedValue(new Error("fixture refresh denied"));
  render(<LegacyImport disabled={false} onImported={imported} />);
  fireEvent.change(screen.getByLabelText("Previous SQLite database"), {
    target: { value: "/old/state.db" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import data" }));
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Data imported, but settings could not be refreshed.");
  expect(alert.textContent).toContain("fixture refresh denied");
  expect(screen.queryByText(/Could not import data\./)).toBeNull();
  fireEvent.click(within(alert).getByRole("button", { name: "Dismiss notice" }));
  expect(screen.getByText("/app/archive")).toBeTruthy();
  expect(screen.getByText("7 runs imported.")).toBeTruthy();
  expect(factoryApi.importLegacy).toHaveBeenCalledTimes(1);
});

it("keeps archived evidence after dismissing a successful import notice", async () => {
  vi.mocked(factoryApi.importLegacy).mockResolvedValue({
    runs_imported: 1,
    settings_imported: true,
    archive_path: "/fixture/import archive",
    warnings: ["fixture warning"],
  });
  render(<LegacyImport disabled={false} onImported={vi.fn()} />);
  fireEvent.change(screen.getByLabelText("Previous YAML config"), {
    target: { value: "/old/config.yaml" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import data" }));
  const status = await screen.findByRole("status");
  expect(status.textContent).toContain("1 run imported. Settings imported.");
  fireEvent.click(within(status).getByRole("button", { name: "Dismiss notice" }));
  expect(screen.getByText("/fixture/import archive")).toBeTruthy();
  expect(screen.getByText("fixture warning")).toBeTruthy();
  expect(screen.queryByRole("status")).toBeNull();
});

it("hides visit feedback while preserving the confirmed report", async () => {
  const view = render(<LegacyImport disabled={false} onImported={vi.fn()} active />);
  fireEvent.change(screen.getByLabelText("Previous SQLite database"), {
    target: { value: "/old/state.db" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Import data" }));
  await screen.findByRole("status");
  view.rerender(<LegacyImport disabled={false} onImported={vi.fn()} active={false} />);
  expect(screen.queryByRole("status")).toBeNull();
  view.rerender(<LegacyImport disabled={false} onImported={vi.fn()} active />);
  expect(screen.queryByRole("status")).toBeNull();
  expect(screen.getByText("/app/archive")).toBeTruthy();
  expect(screen.getByText("7 runs imported.")).toBeTruthy();
});

it("contextualizes and restores focus after a file picker failure", async () => {
  vi.mocked(open).mockRejectedValueOnce(new Error("fixture picker denied"));
  render(<LegacyImport disabled={false} onImported={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Choose YAML config" }));
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Could not choose import file.");
  expect(alert.textContent).toContain("fixture picker denied");
  const close = within(alert).getByRole("button", { name: "Dismiss notice" });
  close.focus();
  fireEvent.click(close);
  expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Import previous daemon" }));
});
