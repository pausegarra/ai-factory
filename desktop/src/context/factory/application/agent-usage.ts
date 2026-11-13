import type { Agent, AgentUsage, UsageEntry, UsageCode } from "../domain/types";
export type UsageViewEntry = { entry: UsageEntry; stale: boolean; error: UsageCode | null };
export type UsageSection = {
  loading: boolean;
  result: AgentUsage | null;
  entries: UsageViewEntry[];
  error: UsageCode | null;
};
export type UsageView = Record<Agent, UsageSection>;
export function emptyUsageView(): UsageView {
  const section = (): UsageSection => ({ loading: true, result: null, entries: [], error: null });
  return { codex: section(), opencode: section() };
}
const valid = (row: UsageViewEntry) => row.entry.observation.status === "available" && row.entry.observation.metrics.length > 0;

/** A single opening owns its snapshot; closing invalidates every pending callback. */
export class AgentUsageSession {
  private active = true;
  private inFlight = false;
  private generation = 0;
  private view = emptyUsageView();
  constructor(private load: (agent: Agent) => Promise<AgentUsage>, private changed: (view: UsageView) => void) {}
  private publish(agent: Agent, section: UsageSection) {
    this.view = { ...this.view, [agent]: section };
    this.changed(this.view);
  }
  async refresh(): Promise<void> {
    if (!this.active || this.inFlight) return;
    this.inFlight = true;
    const ticket = ++this.generation;
    this.view = {
      codex: { ...this.view.codex, loading: true },
      opencode: { ...this.view.opencode, loading: true },
    };
    this.changed(this.view);
    await Promise.allSettled((["codex", "opencode"] as const).map(async agent => {
      try {
        const result = await this.load(agent);
        if (!this.active || ticket !== this.generation) return;
        const previous = this.view[agent];
        if (result.status === "error") {
          this.failed(agent, result.code ?? "query_failed", result);
          return;
        }
        const entries = result.entries.map(entry => {
          const old = previous.entries.find(row => row.entry.id === entry.id);
          return entry.observation.status === "error" && old && valid(old)
            ? { ...old, stale: true, error: entry.observation.code ?? "query_failed" }
            : { entry, stale: false, error: null };
        });
        this.publish(agent, { loading: false, result, entries, error: null });
      } catch {
        if (this.active && ticket === this.generation) this.failed(agent, "query_failed", null);
      }
    }));
    if (this.active && ticket === this.generation) this.inFlight = false;
  }
  private failed(agent: Agent, code: UsageCode, result: AgentUsage | null) {
    const previous = this.view[agent];
    this.publish(agent, {
      loading: false, result: result ?? previous.result, error: code,
      entries: previous.entries.map(row => valid(row) ? { ...row, stale: true, error: code } : row),
    });
  }
  close(): void { this.active = false; ++this.generation; }
}
