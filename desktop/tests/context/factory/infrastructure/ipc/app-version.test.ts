import { beforeEach, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: vi.fn() }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn() }));

import { isTauri } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { readAppVersion } from "../../../../../src/context/factory/infrastructure/ipc/app-version";

beforeEach(() => {
  vi.mocked(isTauri).mockReset().mockReturnValue(true);
  vi.mocked(getVersion).mockReset();
});

it.each(["0.8.0", "4.12.3", "1.2.3-rc.1+fixture"])(
  "returns the runtime version unchanged: %s", async (version) => {
    vi.mocked(getVersion).mockResolvedValue(version);
    await expect(readAppVersion()).resolves.toBe(version);
    expect(getVersion).toHaveBeenCalledTimes(1);
  },
);

it("skips native metadata in browser preview", async () => {
  vi.mocked(isTauri).mockReturnValue(false);
  await expect(readAppVersion()).resolves.toBeNull();
  expect(getVersion).not.toHaveBeenCalled();
});

it("turns a rejected native lookup into absence", async () => {
  vi.mocked(getVersion).mockRejectedValue(new Error("fixture unavailable"));
  await expect(readAppVersion()).resolves.toBeNull();
  expect(getVersion).toHaveBeenCalledTimes(1);
});

it.each(["", "   "])("does not expose an empty version: %j", async (version) => {
  vi.mocked(getVersion).mockResolvedValue(version);
  await expect(readAppVersion()).resolves.toBeNull();
});
