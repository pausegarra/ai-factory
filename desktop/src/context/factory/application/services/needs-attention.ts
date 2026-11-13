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

/** Count unresolved events from the complete visible history without changing it. */
export function countNeedsAttention(runs: readonly Run[]): number {
  const latest = new Map<string, Candidate>();
  for (const run of runs) {
    if (["queued", "running", "cancelled"].includes(run.status)) continue;
    const next = candidate(run);
    const previous = latest.get(run.event_id);
    if (!previous || compare(next, previous) > 0) latest.set(run.event_id, next);
  }
  let count = 0;
  for (const { run } of latest.values()) {
    if (run.status !== "succeeded") count++;
  }
  return count;
}
