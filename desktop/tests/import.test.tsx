import { afterEach, it, expect, vi } from "vitest";
import {
  render,
  screen,
  cleanup,
  fireEvent,
  waitFor,
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
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import { LegacyImport } from "../src/context/factory/infrastructure/presentations/legacy-import";
afterEach(cleanup);
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
