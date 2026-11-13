import type { Run } from "../../domain/types";

type Candidate = { run: Run; milliseconds: number; fraction: string };

function candidate(run: Run): Candidate {
  return {
    run,
    milliseconds: Date.parse(run.created_at),
    // Date.parse truncates sub-millisecond precision from native RFC3339 dates.
    fraction: (run.created_at.match(/\.(\d+)/)?.[1] ?? "").padEnd(9, "0"),
  };
}

function compare(a: Candidate, b: Candidate): number {
  if (a.milliseconds !== b.milliseconds) return a.milliseconds - b.milliseconds;
  if (a.fraction !== b.fraction) return a.fraction < b.fraction ? -1 : 1;
  if (a.run.attempt !== b.run.attempt) return a.run.attempt - b.run.attempt;
  return a.run.id === b.run.id ? 0 : a.run.id < b.run.id ? -1 : 1;
}

/** Count unresolved work from the complete visible history without changing it. */
export function countNeedsAttention(runs: readonly Run[]): number {
  const latest = new Map<string, Candidate>();
  for (const run of runs) {
    if (["queued", "running", "cancelled"].includes(run.status)) continue;
    // Approval events include the head SHA; a later commit can resolve the same PR.
    const key = run.trigger_id === "pr_approved"
      ? JSON.stringify(["pr_approved", run.repository.toLowerCase(), run.number])
      : JSON.stringify(["event", run.event_id]);
    const next = candidate(run);
    const previous = latest.get(key);
    if (!previous || compare(next, previous) > 0) latest.set(key, next);
  }
  let count = 0;
  for (const { run } of latest.values()) {
    if (run.status !== "succeeded") count++;
  }
  return count;
}
