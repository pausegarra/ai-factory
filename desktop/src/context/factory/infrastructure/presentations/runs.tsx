import { useEffect, useRef, useState } from "react";
import {
  ArrowClockwise,
  ArrowLeft,
  ArrowUpRight,
  Clock,
  Code,
  GitBranch,
  MagnifyingGlass,
  Stop,
  Terminal,
  Tray,
  WarningCircle,
} from "@phosphor-icons/react";
import type { Run, RunDetail, Snapshot, Trigger } from "../../domain/types";
import { TRIGGERS } from "../../domain/types";
import { factoryApi } from "../ipc/factory-api";
import { LogReader } from "../../application/log-reader";
import { LiveResource } from "../../application/live-resource";
import { Badge, date, duration, Empty, Notice } from "./ui";
export function RunTable({
  runs,
  onSelect,
}: {
  runs: Run[];
  onSelect: (id: string) => void;
}) {
  return (
    <div className="table-scroll">
      <table className="run-table">
        <thead>
          <tr>
            <th>Task</th>
            <th>Trigger</th>
            <th>Status</th>
            <th>Current step</th>
            <th>Started</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {runs.map((run) => (
            <tr key={run.id} onClick={() => onSelect(run.id)}>
              <td>
                <button
                  className="run-title"
                  onClick={(e) => {
                    e.stopPropagation();
                    onSelect(run.id);
                  }}
                >
                  {run.title}
                </button>
                <span className="run-meta">
                  {run.repository} <span>#{run.number}</span>
                </span>
              </td>
              <td>
                <span className="trigger-label">
                  <GitBranch size={14} />
                  {TRIGGERS[run.trigger_id]?.short || run.trigger_id}
                </span>
              </td>
              <td>
                <Badge status={run.status} />
              </td>
              <td>
                <span className="step-name">{run.current_step || "—"}</span>
              </td>
              <td className="muted whitespace-nowrap">
                {date(run.started_at || run.created_at)}
              </td>
              <td>
                <ArrowUpRight size={16} className="muted" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
export function RunsPage({
  snapshot,
  onSelect,
}: {
  snapshot: Snapshot;
  onSelect: (id: string) => void;
}) {
  const [view, setView] = useState("active");
  const [query, setQuery] = useState("");
  const [trigger, setTrigger] = useState("all");
  const active = snapshot.runs.filter((r) =>
    ["queued", "running"].includes(r.status),
  );
  const runs = snapshot.runs.filter(
    (r) =>
      (view === "all" ||
        (view === "active"
          ? ["queued", "running"].includes(r.status)
          : !["queued", "running"].includes(r.status))) &&
      (trigger === "all" || r.trigger_id === trigger) &&
      `${r.title} ${r.repository} ${r.number} ${r.id}`
        .toLowerCase()
        .includes(query.toLowerCase()),
  );
  return (
    <>
      <div className="page-title">
        <div>
          <span className="eyebrow">OPERATIONS CENTER</span>
          <h1>Runs</h1>
          <p>Every task, every step. All in real time.</p>
        </div>
        <span className="live-label">
          <span />
          Live updates
        </span>
      </div>
      <section className="panel">
        <div className="list-toolbar">
          <div className="segmented">
            {[
              ["active", `Running ${active.length}`],
              ["history", "History"],
              ["all", "All runs"],
            ].map(([id, label]) => (
              <button
                key={id}
                className={view === id ? "selected" : ""}
                onClick={() => setView(id)}
              >
                {label}
              </button>
            ))}
          </div>
          <div className="filters">
            <label className="search-field">
              <MagnifyingGlass size={17} />
              <input
                aria-label="Search runs"
                placeholder="Search task or repository"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </label>
            <select
              aria-label="Filter trigger"
              value={trigger}
              onChange={(e) => setTrigger(e.target.value)}
            >
              <option value="all">All triggers</option>
              {Object.entries(TRIGGERS).map(([id, v]) => (
                <option key={id} value={id}>
                  {v.label}
                </option>
              ))}
            </select>
          </div>
        </div>
        {runs.length ? (
          <RunTable runs={runs} onSelect={onSelect} />
        ) : (
          <Empty
            icon={<Tray size={32} />}
            title={
              query
                ? "No matches found"
                : view === "active"
                  ? "The line is standing by"
                  : "No runs yet"
            }
            description={
              query
                ? "Try another name, number or repository."
                : view === "active"
                  ? "New tasks appear as soon as they are detected. Agents run your workflow automatically."
                  : "Your history appears here after the first task."
            }
          />
        )}
        <div className="panel-footer">
          <span>
            {runs.length} {runs.length === 1 ? "run" : "runs"}
          </span>
          <span>Local SQLite · Event driven updates</span>
        </div>
      </section>
    </>
  );
}
export function RunDetailPage({
  id,
  onBack,
  onSelect,
}: {
  id: string;
  onBack: () => void;
  onSelect: (id: string) => void;
}) {
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [error, setError] = useState("");
  const [nodeId, setNodeId] = useState("");
  const [logs, setLogs] = useState("");
  const [busy, setBusy] = useState(false);
  const [autoScroll, setAutoScroll] = useState(true);
  const logRef = useRef<HTMLPreElement>(null);
  useEffect(() => {
    setDetail(null);
    setNodeId("");
    setError("");
    const resource = new LiveResource(
      {
        listen: (cb) =>
          factoryApi.subscribe((e) => {
            if (e.kind === "resync" || e.run_id === id) cb();
          }),
        load: () => factoryApi.detail(id),
      },
      setDetail,
      (e) => setError(String(e)),
    );
    void resource.start();
    return () => resource.stop();
  }, [id]);
  useEffect(() => {
    if (detail && !detail.nodes.some((n) => n.id === nodeId))
      setNodeId(detail.run.current_step || detail.nodes[0]?.id || "");
  }, [detail, nodeId]);
  useEffect(() => {
    setLogs("");
    if (!nodeId) return;
    const reader = new LogReader((offset) =>
      factoryApi.logs(id, nodeId, offset),
    );
    const resource = new LiveResource(
      {
        listen: (cb) =>
          factoryApi.subscribe((e) => {
            if (
              e.kind === "resync" ||
              (e.run_id === id && (!e.node_id || e.node_id === nodeId))
            )
              cb();
          }),
        load: () => reader.read(),
      },
      setLogs,
      (e) => setError(String(e)),
    );
    void resource.start();
    return () => resource.stop();
  }, [id, nodeId]);
  useEffect(() => {
    if (autoScroll && logRef.current)
      logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [logs, autoScroll]);
  async function action(kind: "cancel" | "retry") {
    setBusy(true);
    setError("");
    try {
      if (kind === "cancel") await factoryApi.cancel(id);
      else onSelect(await factoryApi.retry(id));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  if (!detail)
    return (
      <>
        <button className="text-button mb-6" onClick={onBack}>
          <ArrowLeft />
          Back to runs
        </button>
        {error ? <Notice>{error}</Notice> : <div className="skeleton h-64" />}
      </>
    );
  const { run } = detail;
  const active = ["running", "queued"].includes(run.status);
  const current = detail.nodes.find((n) => n.id === nodeId);
  return (
    <>
      <button className="text-button mb-6" onClick={onBack}>
        <ArrowLeft size={16} />
        All runs
      </button>
      <div className="page-title">
        <div>
          <span className="eyebrow">
            {run.repository} · #{run.number}
          </span>
          <h1 className="run-detail-title">{run.title}</h1>
          <p>
            {TRIGGERS[run.trigger_id]?.label} · Attempt {run.attempt} ·{" "}
            {date(run.created_at)}
          </p>
        </div>
        <div className="flex items-center gap-3">
          <Badge status={run.status} />
          <button
            className="button button-secondary"
            disabled={busy}
            onClick={() => void action(active ? "cancel" : "retry")}
          >
            {active ? <Stop size={16} /> : <ArrowClockwise size={16} />}{" "}
            {busy ? "Processing…" : active ? "Cancel" : "Retry"}
          </button>
        </div>
      </div>
      {error && <Notice close={() => setError("")}>{error}</Notice>}
      {run.error && <Notice>{run.error}</Notice>}
      <div className="detail-grid">
        <section className="panel timeline">
          <div className="panel-heading">
            <h2>Workflow steps</h2>
            <span className="muted">{detail.nodes.length}</span>
          </div>
          <div className="timeline-list">
            {detail.nodes.map((node, i) => (
              <button
                key={node.id}
                className={`timeline-item ${node.id === nodeId ? "selected" : ""}`}
                onClick={() => setNodeId(node.id)}
              >
                <span className={`step-number ${node.status}`}>
                  {String(i + 1).padStart(2, "0")}
                </span>
                <span>
                  <strong>{node.name}</strong>
                  <small>
                    {node.agent} · {duration(node.started_at, node.finished_at)}
                  </small>
                </span>
                <Badge status={node.status} />
              </button>
            ))}
          </div>
          <div className="run-facts">
            <span>
              <Clock size={16} />
              Duration{" "}
              <strong>{duration(run.started_at, run.finished_at)}</strong>
            </span>
            <span>
              <Code size={16} />
              Author <strong>{run.actor}</strong>
            </span>
            <span className="break-all">
              ID <code>{run.id}</code>
            </span>
          </div>
        </section>
        <section className="panel log-panel">
          <div className="panel-heading">
            <div className="flex items-center gap-3">
              <Terminal size={18} />
              <h2>{current?.name || "Session"}</h2>
            </div>
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={autoScroll}
                onChange={(e) => setAutoScroll(e.target.checked)}
              />
              Follow logs
            </label>
          </div>
          <div className="log-meta">
            <span>Session output</span>
            <span>{current?.session_id || "No session started"}</span>
          </div>
          {current?.error && <Notice>{current.error}</Notice>}
          <pre className="log-output" ref={logRef}>
            {logs || "Waiting for agent output…"}
          </pre>
          {current?.result != null && (
            <details className="result-details">
              <summary>Step result</summary>
              <pre>{JSON.stringify(current.result, null, 2)}</pre>
            </details>
          )}
        </section>
      </div>
    </>
  );
}
