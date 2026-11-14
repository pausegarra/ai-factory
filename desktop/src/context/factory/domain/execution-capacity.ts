import type { Settings, Trigger } from "./types";

export function effectiveTriggerLimit(settings: Settings, trigger: Trigger): number {
  return settings.trigger_max_concurrent?.[trigger] ?? settings.max_concurrent;
}

export function withTriggerLimit(
  settings: Settings, trigger: Trigger, limit: number | undefined,
): Settings {
  const overrides = { ...settings.trigger_max_concurrent };
  if (limit === undefined) delete overrides[trigger];
  else overrides[trigger] = limit;
  return { ...settings, trigger_max_concurrent: overrides };
}
