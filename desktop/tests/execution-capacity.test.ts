import { it, expect } from "vitest";
import { validateSettings } from "../src/context/factory/domain/rules";
import { effectiveTriggerLimit, withTriggerLimit } from "../src/context/factory/domain/execution-capacity";
import type { Settings, Trigger } from "../src/context/factory/domain/types";
import { noticeSettings } from "./fixtures/notices";

it("keeps explicit equal values while missing values follow global", () => {
  const base = { ...noticeSettings(), max_concurrent: 8 };
  const explicit = withTriggerLimit(base, "new_issue", 8);
  const changed = { ...explicit, max_concurrent: 4 };
  expect(effectiveTriggerLimit(changed, "new_issue")).toBe(8);
  expect(effectiveTriggerLimit(changed, "pr_review")).toBe(4);
  const inherited = withTriggerLimit(changed, "new_issue", undefined);
  expect(inherited.trigger_max_concurrent).toEqual({});
  expect(effectiveTriggerLimit(inherited, "new_issue")).toBe(4);
  expect(base.trigger_max_concurrent).toBeUndefined();
  expect(explicit.trigger_max_concurrent).toEqual({ new_issue: 8 });
});
it("accepts independent ceilings whose sum exceeds global and preserves other settings", () => {
  const base = { ...noticeSettings(), max_concurrent: 8,
    trigger_max_concurrent: { new_issue: 4, pr_review: 4, pr_approved: 2 } };
  expect(validateSettings(base)).toEqual([]);
  const changed = withTriggerLimit(base, "pr_review", 64);
  expect(changed.trigger_max_concurrent).toEqual({ new_issue: 4, pr_review: 64, pr_approved: 2 });
  expect(changed.flows).toEqual(base.flows);
  expect(changed.commands).toEqual(base.commands);
  expect(base.trigger_max_concurrent.pr_review).toBe(4);
});
for (const field of ["global", "new_issue", "pr_review", "pr_approved"]) {
  it.each([0, -1, 1.5, 65, NaN, Infinity, "4", null])(`rejects invalid ${field} capacity %s`, invalid => {
    const value = noticeSettings();
    if (field === "global") value.max_concurrent = invalid as number;
    else value.trigger_max_concurrent = { [field]: invalid } as Settings["trigger_max_concurrent"];
    expect(validateSettings(value).join(" ")).toMatch(field === "global" ? /concurrency/i : new RegExp(field));
  });
  it.each([1, 64])(`accepts ${field} boundary %s`, limit => {
    const value = field === "global" ? { ...noticeSettings(), max_concurrent: limit }
      : withTriggerLimit(noticeSettings(), field as Trigger, limit);
    expect(validateSettings(value)).toEqual([]);
  });
}
it.each([null, [], "4", { unknown: 4 }, { toString: 4 }].map(overrides => ({ overrides })))("rejects malformed trigger maps $overrides", ({ overrides }) => {
  expect(validateSettings({ ...noticeSettings(), trigger_max_concurrent: overrides } as unknown as Settings).length).toBeGreaterThan(0);
});
