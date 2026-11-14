import { expect, it } from "vitest";
import type { Agent } from "../../../../../src/context/factory/domain/types";
import { usageResetText, usageWindowLabel } from "../../../../../src/context/factory/infrastructure/presentations/usage-window";

const now = Date.parse("2027-01-15T08:00:00Z");
it.each([
  [null, null],
  ["bad-date", null],
  ["2027-01-15T07:59:59Z", "Reset time reached"],
  ["2027-01-15T08:00:00Z", "Reset time reached"],
  ["2027-01-15T08:00:00.001Z", "Resets in <1m"],
  ["2027-01-15T08:00:59.999Z", "Resets in <1m"],
  ["2027-01-15T08:01:00Z", "Resets in 1m"],
  ["2027-01-15T08:59:59Z", "Resets in 59m"],
  ["2027-01-15T09:00:00Z", "Resets in 1h 0m"],
  ["2027-01-15T10:15:00Z", "Resets in 2h 15m"],
  ["2027-01-16T08:00:00Z", "Resets in 1d 0h 0m"],
  ["2027-01-17T11:05:00Z", "Resets in 2d 3h 5m"],
  ["2027-01-15T10:15:00+02:00", "Resets in 15m"],
] as const)("formats reset %s as %s", (reset, expected) => {
  expect(usageResetText(reset, now)).toBe(expected);
});
it("depends on instants across offsets and DST, not on local clock fields", () => {
  const before = Date.parse("2027-03-28T01:30:00+01:00");
  expect(usageResetText("2027-03-28T03:30:00+02:00", before)).toBe("Resets in 1h 0m");
  expect(usageResetText("2027-03-28T01:30:00Z", before)).toBe("Resets in 1h 0m");
});
it("does not render a duration from an invalid current instant", () => {
  expect(usageResetText("2027-01-15T09:00:00Z", Number.NaN)).toBeNull();
});

it.each([
  ["codex", "codex:primary", "Primary window", "300 minutes", "5 hour window"],
  ["codex", "codex:secondary", "Secondary window", "10080 minutes", "Weekly window"],
  ["codex", "codex:secondary", "Secondary window", "300 minutes", "5 hour window"],
  ["codex", "codex:primary", "Primary window", "10080 minutes", "Weekly window"],
  ["codex", "codex:primary", "weekly", null, null],
  ["codex", "codex:primary", "Primary window", "299 minutes", null],
  ["codex", "codex:primary", "Primary window", "301 minutes", null],
  ["codex", "codex:primary", "Primary window", "10079 minutes", null],
  ["codex", "codex:primary", "Primary window", "10081 minutes", null],
  ["codex", "codex:primary", "Primary window", "300 minutes extra", null],
  ["codex", "codex:primary", "Primary window", "300", null],
  ["opencode", "opencode:weekly", "Quota", null, "Weekly window"],
  ["opencode", "weekly", "Quota", null, "Weekly window"],
  ["opencode", "provider:quota", "weekly", null, "Weekly window"],
  ["opencode", "opencode:rolling", "rolling", null, null],
  ["opencode", "opencode:monthly", "monthly", null, null],
  ["opencode", "opencode:rolling", "rolling", "300 minutes", null],
  ["opencode", "opencode:not-weekly", "almost weekly", null, null],
] as const)("classifies only explicit metadata: %s %s %s %s", (agent, id, label, period, expected) => {
  expect(usageWindowLabel(agent satisfies Agent, { id, label, period })).toBe(expected);
});
