import {
  ArrowRight,
  ArrowUpRight,
  CheckCircle,
  Clock,
  GitBranch,
  Lightning,
  Play,
  Plus,
  Queue,
  Robot,
  Terminal,
  Tray,
  WarningCircle,
} from "@phosphor-icons/react";
import type { Settings, Snapshot } from "../../domain/types";
import { TRIGGERS } from "../../domain/types";
import { RunTable } from "./runs";
import { date, Empty } from "./ui";
export function OverviewPage({
  snapshot,
  settings,
  onPage,
  onRun,
}: {
  snapshot: Snapshot;
  settings: Settings;
  onPage: (page: string) => void;
  onRun: (id: string) => void;
}) {
  const configured =
    settings.token_profiles.some((t) => t.has_token && t.owners.length > 0);
  const { metrics } = snapshot;
  const active = snapshot.runs.filter((r) =>
    ["running", "queued"].includes(r.status),
  );
  const recent = snapshot.runs.slice(0, 5);
  const limit = settings.max_concurrent;
  return (
    <>
      <div className="page-title">
        <div>
          <span className="eyebrow">YOUR TEAM, IN MOTION</span>
          <h1>
            Factory overview<span className="heading-dot">.</span>
          </h1>
          <p>From the first issue to the final merge, all connected.</p>
        </div>
        <span className="today">
          {new Intl.DateTimeFormat("en-US", {
            day: "numeric",
            month: "long",
            year: "numeric",
          }).format(new Date())}
        </span>
      </div>
      <div className="metrics-grid">
        {[
          {
            label: "Running",
            value: metrics.running,
            icon: <Lightning size={20} />,
            note: `of ${limit} available slots`,
            tone: "orange",
          },
          {
            label: "Queued",
            value: metrics.queued,
            icon: <Queue size={20} />,
            note: "waiting for their turn",
            tone: "",
          },
          {
            label: "Completed",
            value: metrics.succeeded,
            icon: <CheckCircle size={20} />,
            note: "tasks finished",
            tone: "",
          },
          {
            label: "Need attention",
            value: metrics.failed + metrics.interrupted,
            icon: <WarningCircle size={20} />,
            note: "failed or interrupted",
            tone: "",
          },
        ].map((m) => (
          <section className={`metric ${m.tone}`} key={m.label}>
            <div className="metric-top">
              <span>{m.label}</span>
              {m.icon}
            </div>
            <strong>{m.value.toString().padStart(2, "0")}</strong>
            <span className="metric-note">{m.note}</span>
          </section>
        ))}
      </div>
      {!configured && (
        <section className="setup-banner">
          <div className="setup-symbol">
            <Robot size={32} />
          </div>
          <div>
            <span className="eyebrow">START WITH THE ESSENTIALS</span>
            <h2>Set up your first production line</h2>
            <p>Connect GitHub, configure permissions and put your agents to work.</p>
            <div className="setup-checklist">
              <span
                className={
                  settings.token_profiles.some((t) => t.has_token) ? "done" : ""
                }
              >
                <CheckCircle size={16} />
                GitHub account
              </span>
              <span className={configured ? "done" : ""}>
                <CheckCircle size={16} />
                Owners and permissions
              </span>
            </div>
          </div>
          <button
            className="button button-primary"
            onClick={() => onPage("settings")}
          >
            Set up Factory
            <ArrowRight size={17} />
          </button>
        </section>
      )}
      <div className="overview-grid">
        <section className="panel activity-panel">
          <div className="panel-heading">
            <div className="flex items-center gap-3">
              <span className="status-dot" />
              <h2>
                {active.length ? "On the production line" : "Recent activity"}
              </h2>
            </div>
            <button className="text-button" onClick={() => onPage("runs")}>
              View runs
              <ArrowUpRight size={15} />
            </button>
          </div>
          {recent.length ? (
            <RunTable
              runs={active.length ? active.slice(0, 5) : recent}
              onSelect={onRun}
            />
          ) : (
            <Empty
              icon={<Tray size={32} />}
              title="Ready for the first task"
              description="Open an issue with [AI] in a connected repository. Your workflow starts automatically."
            />
          )}
          <div className="panel-footer">
            <span className="live-label">
              <span />
              Listening for run events
            </span>
            <span>{metrics.total} total tasks</span>
          </div>
        </section>
        <section className="panel capacity-panel">
          <div className="panel-heading">
            <h2>Capacity</h2>
            <span className="muted">GLOBAL</span>
          </div>
          <div className="capacity-value">
            <strong>{metrics.running}</strong>
            <span>/ {limit}</span>
          </div>
          <div className="capacity-slots">
            {Array.from({ length: Math.min(limit, 16) }, (_, i) => (
              <span key={i} className={i < metrics.running ? "used" : ""} />
            ))}
          </div>
          <p>
            {metrics.running
              ? `${Math.max(0, limit - metrics.running)} slots available for new tasks.`
              : "All execution slots are available."}
          </p>
          <div className="capacity-agents">
            <span>
              <span className="agent-mini">C</span>Codex <small>CLI</small>
            </span>
            <span>
              <span className="agent-mini opencode">O</span>OpenCode{" "}
              <small>CLI</small>
            </span>
          </div>
          <button className="text-button" onClick={() => onPage("settings")}>
            Adjust concurrency
            <ArrowRight size={15} />
          </button>
        </section>
      </div>
      <div className="section-label">
        <div>
          <span className="eyebrow">AUTOMATIONS</span>
          <h2>Three entry points. One team.</h2>
        </div>
        <button className="text-button" onClick={() => onPage("flows")}>
          Edit workflows
          <ArrowUpRight size={15} />
        </button>
      </div>
      <div className="flow-summary-grid">
        {settings.flows.map((flow, i) => (
          <button
            className="flow-summary"
            key={flow.id}
            onClick={() => onPage("flows")}
          >
            <div className="flex items-center justify-between">
              <span className="flow-summary-icon">
                <GitBranch size={21} />
              </span>
              <span className={`flow-state ${flow.enabled ? "enabled" : ""}`}>
                <span />
                {flow.enabled ? "Active" : "Paused"}
              </span>
            </div>
            <div className="flow-summary-title">
              <span>0{i + 1}</span>
              <h3>{TRIGGERS[flow.id]?.label || flow.name}</h3>
            </div>
            <p>{TRIGGERS[flow.id]?.description}</p>
            <div className="flow-summary-footer">
              <span>
                {flow.nodes.length} {flow.nodes.length === 1 ? "step" : "steps"}{" "}
                · {new Set(flow.nodes.map((n) => n.agent)).size}{" "}
                {new Set(flow.nodes.map((n) => n.agent)).size === 1
                  ? "agent"
                  : "agents"}
              </span>
              <ArrowRight size={17} />
            </div>
          </button>
        ))}
      </div>
      <div className="overview-footer">
        <span>
          <Clock size={14} />
          Last GitHub check: {date(snapshot.service.last_poll_at)}
        </span>
        <span>AI Factory · Build with intent.</span>
      </div>
    </>
  );
}
