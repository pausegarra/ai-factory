import type { ExecutionCapacity, Trigger } from "../../src/context/factory/domain/types";

export function capacityFixture(
  active: Partial<Record<Trigger, number>> = {},
  limit = 4,
  overrides: Partial<Record<Trigger, number>> = {},
): ExecutionCapacity {
  const usage = (trigger: Trigger) => ({
    active: active[trigger] ?? 0, limit: overrides[trigger] ?? limit,
  });
  const by_trigger = {
    new_issue: usage("new_issue"),
    pr_review: usage("pr_review"),
    pr_approved: usage("pr_approved"),
  };
  return {
    global: { active: Object.values(by_trigger).reduce((sum, v) => sum + v.active, 0), limit },
    by_trigger,
  };
}
