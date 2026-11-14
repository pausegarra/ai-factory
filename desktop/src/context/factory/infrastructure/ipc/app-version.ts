import { getVersion } from "@tauri-apps/api/app";
import { isTauri } from "@tauri-apps/api/core";

export async function readAppVersion(): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    const version = await getVersion();
    return version.trim() === "" ? null : version;
  } catch {
    return null;
  }
}
