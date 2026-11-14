import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { deferred, noticeSettings, noticeSnapshot } from "../../../../fixtures/notices";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn() }));
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: { settings: vi.fn(), snapshot: vi.fn(), subscribe: vi.fn() },
}));

import { getVersion } from "@tauri-apps/api/app";
import { factoryApi } from "../../../../../src/context/factory/infrastructure/ipc/factory-api";
import { App } from "../../../../../src/context/factory/infrastructure/presentations/app";

beforeEach(() => {
  vi.mocked(getVersion).mockReset().mockResolvedValue("0.8.0");
  vi.mocked(factoryApi.settings).mockResolvedValue(noticeSettings());
  vi.mocked(factoryApi.snapshot).mockResolvedValue(noticeSnapshot());
  vi.mocked(factoryApi.subscribe).mockResolvedValue(() => {});
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

function versionSlot() {
  const footer = screen.getByText("LOCAL FIRST").closest(".sidebar-footer");
  expect(footer).not.toBeNull();
  expect(footer!.children).toHaveLength(2);
  return footer!.children[1];
}

it.each(["0.8.0", "4.12.3", "1.2.3-rc.1+fixture"])(
  "shows the confirmed runtime version in the footer: %s", async (version) => {
    vi.mocked(getVersion).mockResolvedValue(version);
    render(<App />);
    const label = await screen.findByText(`v${version}`, { exact: true });
    expect(label).toBe(versionSlot());
    expect(screen.queryByText("v0.2.0")).toBeNull();
    expect(getVersion).toHaveBeenCalledTimes(1);
  },
);

it("renders Factory while version is pending and fills only the version slot", async () => {
  const lookup = deferred<string>();
  vi.mocked(getVersion).mockReturnValue(lookup.promise);
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  expect(versionSlot().textContent).toBe("");
  await act(async () => { lookup.resolve("0.8.0"); await lookup.promise; });
  expect(versionSlot().textContent).toBe("v0.8.0");
  expect(screen.getByRole("main")).toBeTruthy();
});

it("keeps Factory functional when native version lookup fails", async () => {
  const lookup = deferred<string>();
  vi.mocked(getVersion).mockReturnValue(lookup.promise);
  render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  await act(async () => {
    lookup.reject(new Error("fixture metadata unavailable"));
    await lookup.promise.catch(() => {});
  });
  expect(versionSlot().textContent).toBe("");
  expect(screen.getByRole("heading", { name: "Factory overview." })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Runs" })).toBeTruthy();
  expect(screen.queryByText(/fixture metadata unavailable/)).toBeNull();
  expect(getVersion).toHaveBeenCalledTimes(1);
});

it("keeps a late lookup from a previous mount separate from the current app", async () => {
  const oldLookup = deferred<string>();
  vi.mocked(getVersion).mockReturnValueOnce(oldLookup.promise);
  const first = render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  first.unmount();
  vi.mocked(getVersion).mockResolvedValueOnce("4.12.3");
  render(<App />);
  await screen.findByText("v4.12.3", { exact: true });
  await act(async () => { oldLookup.resolve("0.8.0"); await oldLookup.promise; });
  expect(versionSlot().textContent).toBe("v4.12.3");
  expect(screen.queryByText("v0.8.0")).toBeNull();
});
