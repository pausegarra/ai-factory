import type { Agent, UsageMetric } from "../../domain/types";

export type UsageWindowLabel = "5 hour window" | "Weekly window";

export function usageWindowLabel(
  agent: Agent,
  metric: Pick<UsageMetric, "id" | "label" | "period">,
): UsageWindowLabel | null {
  if (agent === "codex") {
    if (metric.period === "300 minutes") return "5 hour window";
    if (metric.period === "10080 minutes") return "Weekly window";
  }
  if (agent === "opencode" && (
    metric.id === "opencode:weekly" || metric.id === "weekly" || metric.label === "weekly"
  )) return "Weekly window";
  return null;
}

export function usageResetText(resetsAt: string | null, nowMs: number): string | null {
  if (resetsAt === null || !Number.isFinite(nowMs)) return null;
  const resetMs = Date.parse(resetsAt);
  if (!Number.isFinite(resetMs)) return null;
  const difference = resetMs - nowMs;
  if (difference <= 0) return "Reset time reached";
  const minutes = Math.floor(difference / 60000);
  if (minutes === 0) return "Resets in <1m";
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const remainder = minutes % 60;
  if (days > 0) return `Resets in ${days}d ${hours}h ${remainder}m`;
  if (hours > 0) return `Resets in ${hours}h ${remainder}m`;
  return `Resets in ${remainder}m`;
}
