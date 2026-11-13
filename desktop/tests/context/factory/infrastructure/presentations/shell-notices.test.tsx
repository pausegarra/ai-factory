import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FactoryEvent, Snapshot } from "../../../../../src/context/factory/domain/types";
import { deferred, noticeSettings, noticeSnapshot } from "../../../../fixtures/notices";

const mocks = vi.hoisted(() => ({ listeners: new Set<(event: FactoryEvent) => void>() }));
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({
  native: true,
  factoryApi: {
    settings: vi.fn(), snapshot: vi.fn(), poll: vi.fn(), subscribe: vi.fn(),
    saveSettings: vi.fn(), importLegacy: vi.fn(), agentAccounts: vi.fn(),
    agentModels: vi.fn(), installedSkills: vi.fn(),
  },
}));

import { factoryApi } from "../../../../../src/context/factory/infrastructure/ipc/factory-api";
import { App } from "../../../../../src/context/factory/infrastructure/presentations/app";

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listeners.clear();
  vi.mocked(factoryApi.settings).mockResolvedValue(noticeSettings());
  vi.mocked(factoryApi.snapshot).mockResolvedValue(noticeSnapshot());
  vi.mocked(factoryApi.poll).mockResolvedValue(undefined);
  vi.mocked(factoryApi.saveSettings).mockImplementation(async (value) => value);
  vi.mocked(factoryApi.importLegacy).mockResolvedValue({
    runs_imported: 1,
    settings_imported: true,
    archive_path: "/fixture/archive",
    warnings: [],
  });
  vi.mocked(factoryApi.agentAccounts).mockResolvedValue([]);
  vi.mocked(factoryApi.agentModels).mockResolvedValue([]);
  vi.mocked(factoryApi.installedSkills).mockResolvedValue([]);
  vi.mocked(factoryApi.subscribe).mockImplementation(async (callback) => {
    mocks.listeners.add(callback);
    return () => mocks.listeners.delete(callback);
  });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

afterEach(() => {
  cleanup();
  mocks.listeners.clear();
});

async function showApp() {
  const view = render(<App />);
  await screen.findByRole("heading", { name: "Factory overview." });
  return view;
}

async function resync() {
  await act(async () => {
    mocks.listeners.forEach((callback) =>
      callback({ kind: "resync", timestamp: "fixture" }),
    );
  });
}

it("keeps a failed Sync now message after a successful data read", async () => {
  vi.mocked(factoryApi.poll).mockRejectedValueOnce(new Error("denied"));
  await showApp();
  fireEvent.click(screen.getByRole("button", { name: "Sync now" }));
  const message = await screen.findByText(/GitHub sync failed\./);
  expect(message.textContent).toContain("denied");
  await resync();
  expect(screen.getByText(/GitHub sync failed\./).textContent).toContain("denied");
});

it("returns focus to main when a service snapshot disables Sync before dismissal", async () => {
  vi.mocked(factoryApi.poll).mockRejectedValueOnce(new Error("denied"));
  await showApp();
  fireEvent.click(screen.getByRole("button", { name: "Sync now" }));
  const message = await screen.findByText(/GitHub sync failed\./);

  const pollingSnapshot = noticeSnapshot();
  pollingSnapshot.service.polling = true;
  vi.mocked(factoryApi.snapshot).mockResolvedValueOnce(pollingSnapshot);
  await resync();
  const syncButton = screen.getByRole("button", { name: "Syncing…" });
  await waitFor(() => expect(syncButton.hasAttribute("disabled")).toBe(true));

  const close = withinNoticeClose(message.closest('[role="alert"]') as HTMLElement)!;
  close.focus();
  await userEvent.setup().keyboard("{Enter}");

  expect(screen.queryByText(/GitHub sync failed\./)).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole("main"));
});

it("shows matching Sync and service diagnostics once with neutral service attribution", async () => {
  vi.mocked(factoryApi.snapshot).mockResolvedValue(noticeSnapshot("denied"));
  vi.mocked(factoryApi.poll).mockRejectedValueOnce(new Error(" denied "));
  await showApp();
  fireEvent.click(screen.getByRole("button", { name: "Sync now" }));
  await screen.findByText(/Factory reported a problem\./);
  await waitFor(() => {
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });
  const persistent = screen.getByRole("alert");
  expect(persistent.textContent).toContain("Factory reported a problem.\ndenied");
  expect(withinNoticeClose(persistent)).toBeNull();
  expect(screen.queryByText(/GitHub sync failed\./)).toBeNull();
});

function withinNoticeClose(notice: HTMLElement) {
  return notice.querySelector<HTMLButtonElement>('button[aria-label="Dismiss notice"]');
}

it("keeps load, Sync, and service errors independent and dismissible by cause", async () => {
  vi.mocked(factoryApi.snapshot).mockResolvedValueOnce(noticeSnapshot("service B"));
  vi.mocked(factoryApi.poll).mockRejectedValueOnce(new Error("sync A"));
  await showApp();
  vi.mocked(factoryApi.snapshot).mockRejectedValueOnce(new Error("load C"));
  fireEvent.click(screen.getByRole("button", { name: "Sync now" }));
  await screen.findByText(/GitHub sync failed\./);
  await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(3));
  expect(screen.getByText(/Factory reported a problem\./).textContent).toContain("service B");
  expect(screen.getByText(/GitHub sync failed\./).textContent).toContain("sync A");
  expect(screen.getByText(/Could not load Factory data\./).textContent).toContain("load C");

  const load = screen.getByText(/Could not load Factory data\./).closest('[role="alert"]') as HTMLElement;
  fireEvent.click(withinNoticeClose(load)!);
  const sync = screen.getByText(/GitHub sync failed\./).closest('[role="alert"]') as HTMLElement;
  fireEvent.click(withinNoticeClose(sync)!);
  expect(screen.queryByText(/Could not load Factory data\./)).toBeNull();
  expect(screen.queryByText(/GitHub sync failed\./)).toBeNull();
  expect(screen.getByText(/Factory reported a problem\./).textContent).toContain("service B");
});

it("allows a new load failure to reappear after the previous one was dismissed", async () => {
  await showApp();
  vi.mocked(factoryApi.snapshot).mockRejectedValueOnce(new Error("first load"));
  await resync();
  const first = await screen.findByText(/Could not load Factory data\./);
  fireEvent.click(withinNoticeClose(first.closest('[role="alert"]')! as HTMLElement)!);
  expect(screen.queryByText(/Could not load Factory data\./)).toBeNull();
  vi.mocked(factoryApi.snapshot).mockRejectedValueOnce(new Error("second load"));
  await resync();
  expect((await screen.findByText(/Could not load Factory data\./)).textContent).toContain("second load");
});

it("does not reload or update a new App when an old Sync settles", async () => {
  const sync = deferred<void>();
  vi.mocked(factoryApi.poll).mockReturnValue(sync.promise);
  const old = await showApp();
  fireEvent.click(screen.getByRole("button", { name: "Sync now" }));
  old.unmount();
  vi.mocked(factoryApi.poll).mockResolvedValue(undefined);
  const current = await showApp();
  const snapshotCalls = vi.mocked(factoryApi.snapshot).mock.calls.length;
  await act(async () => sync.resolve(undefined));
  expect(vi.mocked(factoryApi.snapshot)).toHaveBeenCalledTimes(snapshotCalls);
  expect(screen.queryByText(/GitHub sync failed\./)).toBeNull();
  current.unmount();
});

it("keeps the import report in the same Settings visit when profile IDs refresh", async () => {
  const original = noticeSettings();
  original.token_profiles = [{ id: "before-import", label: "Before", account: "worker", has_token: true, owners: [], authors: [], reviewers: [] }];
  const imported = structuredClone(original);
  imported.token_profiles[0] = { ...imported.token_profiles[0], id: "after-import", label: "After" };
  vi.mocked(factoryApi.settings).mockResolvedValueOnce(original).mockResolvedValue(imported);
  vi.mocked(factoryApi.importLegacy).mockResolvedValue({
    runs_imported: 2,
    settings_imported: true,
    archive_path: "/fixture/profile archive",
    warnings: ["fixture import warning"],
  });
  await showApp();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.change(screen.getByLabelText("Previous SQLite database"), { target: { value: "/old/state.db" } });
  fireEvent.click(screen.getByRole("button", { name: "Import data" }));
  await screen.findByText("/fixture/profile archive");
  await act(async () => {
    mocks.listeners.forEach((callback) => callback({ kind: "service_changed", timestamp: "fixture" }));
  });
  await waitFor(() => expect(factoryApi.settings).toHaveBeenCalledTimes(3));
  expect(screen.getByRole("heading", { name: "Settings" })).toBeTruthy();
  expect(screen.getByText("/fixture/profile archive")).toBeTruthy();
  expect(screen.getByText("fixture import warning")).toBeTruthy();
  expect(screen.getByRole("status").textContent).toContain("2 runs imported. Settings imported.");
  expect(screen.getByLabelText("Import report").textContent).toContain("2 runs imported. Settings imported.");
});

it("preserves partially typed scope separators across equivalent live refreshes", async () => {
  const original = noticeSettings();
  const refreshedSettings = deferred<ReturnType<typeof noticeSettings>>();
  original.token_profiles = [{
    id: "profile", label: "Work", account: "worker", has_token: true,
    owners: ["org"], authors: ["alice"], reviewers: ["bob"],
  }];
  vi.mocked(factoryApi.settings)
    .mockResolvedValueOnce(structuredClone(original))
    .mockReturnValueOnce(refreshedSettings.promise);
  await showApp();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("tab", { name: "GitHub accounts" }));

  const owners = screen.getByLabelText("GitHub owners") as HTMLInputElement;
  const authors = screen.getByLabelText("Allowed authors") as HTMLInputElement;
  const reviewers = screen.getByLabelText("Allowed reviewers") as HTMLInputElement;
  fireEvent.change(owners, { target: { value: "org, " } });
  fireEvent.change(authors, { target: { value: "alice, " } });
  fireEvent.change(reviewers, { target: { value: "bob, " } });

  await resync();
  await waitFor(() => expect(factoryApi.settings).toHaveBeenCalledTimes(2));
  await act(async () => refreshedSettings.resolve(structuredClone(original)));
  expect(owners.value).toBe("org, ");
  expect(authors.value).toBe("alice, ");
  expect(reviewers.value).toBe("bob, ");

  fireEvent.change(owners, { target: { value: "org, another" } });
  fireEvent.change(authors, { target: { value: "alice, another" } });
  fireEvent.change(reviewers, { target: { value: "bob, another" } });
  expect(owners.value).toBe("org, another");
  expect(authors.value).toBe("alice, another");
  expect(reviewers.value).toBe("bob, another");
});
