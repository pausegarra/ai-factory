import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Agent, UsageCode, UsageMetric } from "../../domain/types";
import { AgentUsageSession, emptyUsageView, type UsageSection } from "../../application/agent-usage";
import { factoryApi } from "../ipc/factory-api";

function message(code: UsageCode | null, agent: Agent): string {
  switch (code) {
    case "desktop_required": return "Open AI Factory desktop to view agent usage.";
    case "sign_in_pending": return "Sign-in pending. Usage cannot be queried yet.";
    case "sign_in_failed": return "Sign-in failed. Usage is unavailable for this account.";
    case "not_connected": return agent === "opencode"
      ? "No OpenCode Console account is signed in. Limits for other providers are not reported here."
      : "This account is not connected. Usage is unavailable.";
    case "unsupported": return "This CLI version does not expose compatible quota data.";
    case "no_quota_data": return "No compatible quota data was reported.";
    case "cli_missing": return "The agent CLI could not be found. Check its path in Settings.";
    case "timed_out": return "The usage query timed out. Try Refresh.";
    case "home_unavailable": return "This account's managed home is unavailable. Check the account in Settings.";
    case "stopping": return "Factory is stopping. Usage is unavailable.";
    default: return "Could not query usage. Try Refresh.";
  }
}
function isRemainingPercent(metric: UsageMetric): boolean {
  return metric.kind === "remaining"
    && metric.unit === "percent"
    && Number.isFinite(metric.value)
    && metric.value >= 0
    && metric.value <= 100
    && (metric.resets_at === null || Number.isFinite(Date.parse(metric.resets_at)));
}
function Metric({ metric, agent, accountLabel }: {
  metric: UsageMetric;
  agent: Agent;
  accountLabel: string;
}) {
  const agentLabel = agent === "codex" ? "Codex" : "OpenCode";
  return <div className="usage-metric">
    <strong>{metric.label}</strong>
    <span>Remaining: {metric.value}%</span>
    <div className="usage-progress" role="progressbar"
      aria-label={agentLabel + " " + accountLabel + " " + metric.label + " remaining"}
      aria-valuemin={0} aria-valuemax={100} aria-valuenow={metric.value}>
      <span className="usage-progress-fill" aria-hidden="true"
        style={{ width: metric.value + "%" }} />
    </div>
    {metric.resets_at !== null &&
      <time dateTime={metric.resets_at}>
        Resets {new Date(metric.resets_at).toLocaleString()}
      </time>}
  </div>;
}
function Section({ agent, section }: { agent: Agent; section: UsageSection }) {
  const code = section.error ?? section.result?.code ?? null;
  return <section className="usage-section" aria-labelledby={`usage-${agent}`} aria-busy={section.loading}>
    <h3 id={`usage-${agent}`}>{agent === "codex" ? "Codex" : "OpenCode"}</h3>
    {section.loading && <div role="status"><span className="skeleton usage-skeleton" aria-hidden="true" />Checking usage…</div>}
    {section.error && <p role="alert">{message(section.error, agent)}</p>}
    {!section.loading && !section.error && section.result?.status === "no_accounts" && <p>No Codex accounts configured.</p>}
    {!section.loading && !section.error && code && <p role={section.result?.status === "error" ? "alert" : "status"}>{message(code, agent)}</p>}
    {!section.loading && code === "unsupported" && agent === "opencode" && <p>The Console session could not be determined from this interface.</p>}
    <ul className="usage-accounts">
      {section.entries.map(({ entry, stale, error }) => {
        const metrics = entry.observation.status === "available"
          ? entry.observation.metrics.filter(isRemainingPercent)
          : [];
        return <li key={entry.id} className="usage-account">
          <h4>{entry.label}</h4>
          {stale && <p role="alert">Previous data. Refresh failed.</p>}
          {error && !section.error && <p role="alert">{message(error, agent)}</p>}
          {entry.observation.code && <p
            role={entry.observation.status === "error" || entry.observation.status === "failed" ? "alert" : "status"}>
            {message(entry.observation.code, agent)}
          </p>}
          {metrics.map(metric => <Metric key={metric.id} metric={metric}
            agent={agent} accountLabel={entry.label} />)}
          {metrics.length === 0 && !entry.observation.code && <p role="status">
            {agent === "opencode"
              ? "Remaining percentage is not available from this OpenCode response."
              : "Remaining percentage is not available for this account."}
          </p>}
        </li>;
      })}
    </ul>
  </section>;
}

export function AgentUsageDialog({ onClose }: { onClose: () => void }) {
  const [view, setView] = useState(emptyUsageView);
  const session = useRef<AgentUsageSession | null>(null);
  const dialog = useRef<HTMLElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const active = new AgentUsageSession(factoryApi.agentUsage, setView);
    session.current = active;
    void active.refresh();
    return () => { active.close(); if (session.current === active) session.current = null; };
  }, []);
  useLayoutEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const node = dialog.current;
    close.current?.focus();
    const contain = (event: FocusEvent) => { if (event.target instanceof Node && !node?.contains(event.target)) close.current?.focus(); };
    document.addEventListener("focusin", contain);
    return () => {
      document.removeEventListener("focusin", contain);
      // Wait for the shell to remove inert. A StrictMode effect replay still has
      // the same connected dialog and must not restore background focus.
      queueMicrotask(() => {
        if (node?.isConnected) return;
        const target = opener?.isConnected && !(opener instanceof HTMLButtonElement && opener.disabled) ? opener : document.querySelector<HTMLElement>("main");
        target?.focus();
      });
    };
  }, []);
  const busy = view.codex.loading || view.opencode.loading;
  return <div className="dialog-backdrop" onKeyDown={event => {
    if (event.key === "Escape") { event.preventDefault(); onClose(); }
    if (event.key === "Tab") {
      const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const first = buttons[0], last = buttons.at(-1);
      if (!first || !last) { event.preventDefault(); return; }
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    }
  }}>
    <section ref={dialog} role="dialog" aria-modal="true" aria-labelledby="usage-heading" className="confirm-dialog usage-dialog">
      <div className="usage-heading"><h2 id="usage-heading">Agent usage</h2>
        <button ref={close} type="button" className="button button-secondary" aria-label="Close usage" onClick={onClose}>Close</button>
      </div>
      <Section agent="codex" section={view.codex} />
      <Section agent="opencode" section={view.opencode} />
      <div className="dialog-actions">
        <button type="button" className="button button-secondary" disabled={busy} onClick={() => void session.current?.refresh()}>Refresh</button>
      </div>
    </section>
  </div>;
}
