import { useEffect, useMemo, useRef, useState, type Ref } from "react";
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
import type { Run, RunDetail, RunView, Snapshot } from "../../domain/types";
import { TRIGGERS } from "../../domain/types";
import type { LogViewMode } from "../../application/dto/run-log";
import { factoryApi } from "../ipc/factory-api";
import { orderRunSteps } from "../../application/run-step-order";
import { LiveResource } from "../../application/live-resource";
import { Badge, date, duration, Empty, Notice } from "./ui";
import { DeleteRunDialog } from "./delete-run-dialog";
import { RunLogOutput } from "./run-log-output";
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
  headingRef,
}: {
  snapshot: Snapshot;
  onSelect: (id: string) => void;
  headingRef?: Ref<HTMLHeadingElement>;
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
          <h1 ref={headingRef} tabIndex={-1}>Runs</h1>
          <p>Every task, every step. All in real time.</p>
        </div>
        <span className="live-label">
          <span />
          Live updates
        </span>
      </div>
      {snapshot.deletions.length > 0 && <section className="panel" aria-label="Pending deletions">
        <div className="panel-heading"><h2>Pending deletions</h2></div>
        <p>Some files may already be deleted. There is no restoration. Retry deletion to finish cleanup.</p>
        {snapshot.deletions.map(d => <div key={d.run_id} className="panel-heading">
          <div><strong>{d.title}</strong><p>{d.repository} #{d.number} · Attempt {d.attempt}</p><code>{d.run_id}</code>
          {d.last_error && <p>{d.last_error}</p>}</div>
          <button className="button button-secondary" onClick={() => onSelect(d.run_id)}>Open cleanup</button>
        </div>)}
      </section>}
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
interface RunDetailPageProps {
  id: string;
  onBack: () => void;
  onSelect: (id: string) => void;
  onDeleted?: (id: string) => void;
  onUnavailable?: (id: string) => void;
  onDeletionStarted?: (id: string) => void;
  onDeletionSettled?: (id: string) => void;
}

export function RunDetailPage(props: RunDetailPageProps) {
  return <RunDetailContent key={props.id} {...props} />;
}

function RunDetailContent({
  id,
  onBack,
  onSelect,
  onDeleted,
  onUnavailable,
  onDeletionStarted,
  onDeletionSettled,
}: RunDetailPageProps) {
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [view, setView] = useState<RunView | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const callbacks = useRef({ onDeleted, onUnavailable });
  callbacks.current = { onDeleted, onUnavailable };
  const detailResource = useRef<LiveResource<RunView> | null>(null);
  const progress = useRef<HTMLParagraphElement>(null);
  const [error, setError] = useState("");
  const [nodeId, setNodeId] = useState("");
  const [logView, setLogView] = useState<LogViewMode>("readable");
  const [busy, setBusy] = useState(false);
  const [autoScroll, setAutoScroll] = useState(true);
  const visibleNodes = useMemo(() => detail ? orderRunSteps(detail) : [], [detail]);
  useEffect(() => {
    mounted.current = true;
    setDetail(null);
    setView(null);
    setNodeId("");
    setError("");
    const resource = new LiveResource(
      {
        listen: (cb) =>
          factoryApi.subscribe((e) => {
            if (e.run_id === id && ["run_deleted", "run_deletion_pending", "run_deletion_failed"].includes(e.kind)) {
              setDetail(null); setNodeId(""); setConfirm(false);
              if (e.kind === "run_deleted" && !inFlight.current) callbacks.current.onUnavailable?.(id);
            }
            if (e.kind === "resync" || e.run_id === id) cb();
          }),
        load: () => factoryApi.detail(id),
      },
      (next) => {
        setView(next);
        setDetail(next.state === "available" ? next.detail : null);
        if (next.state !== "available") { setNodeId(""); setConfirm(false); }
        if (next.state === "missing" && !inFlight.current) callbacks.current.onUnavailable?.(id);
      },
      (e) => setError(String(e)),
    );
    detailResource.current = resource;
    void resource.start();
    return () => { mounted.current = false; resource.stop(); };
  }, [id]);
  useEffect(() => {
    if (!detail) return;
    setNodeId(selectedId => {
      if (visibleNodes.some(node => node.id === selectedId)) return selectedId;
      const currentId = detail.run.current_step;
      return visibleNodes.find(node => node.id === currentId)?.id ?? visibleNodes[0]?.id ?? "";
    });
  }, [detail, visibleNodes]);
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
  async function remove() {
    if (inFlight.current) return;
    inFlight.current = true;
    onDeletionStarted?.(id);
    setConfirm(false); setDeleting(true); setError("");
    try {
      const outcome = await factoryApi.deleteRun(id);
      if (outcome.state === "deleted") callbacks.current.onDeleted?.(id);
      else if (mounted.current) {
          setView({ state: "deleting", deletion: outcome.deletion }); setDetail(null); setNodeId("");
      }
    } catch (e) {
      // A transport error does not establish whether the irreversible commit ran.
      try {
        const next = await factoryApi.detail(id);
        if (next.state === "missing") callbacks.current.onDeleted?.(id);
        else if (mounted.current) {
          setView(next); setDetail(next.state === "available" ? next.detail : null);
          setError(next.state === "available" ? String(e) : "");
        }
      } catch { if (mounted.current) setError(String(e)); }
    } finally {
      inFlight.current = false;
      onDeletionSettled?.(id);
      if (mounted.current) setDeleting(false);
    }
  }
  useEffect(() => { if (deleting) progress.current?.focus(); }, [deleting, !!detail, view?.state]);
  if (view?.state === "deleting") return <>
    <button className="text-button mb-6" onClick={onBack}>Back to runs</button>
    <h1>Deletion pending</h1><p>{view.deletion.title}</p>
    <p>{view.deletion.repository} #{view.deletion.number} · Attempt {view.deletion.attempt}</p><code>{id}</code>
    <Notice>{view.deletion.last_error || "Cleanup is pending."} Some files may already be deleted. There is no restoration.</Notice>
    {error && <Notice>{error}</Notice>}
    <p ref={progress} tabIndex={-1} role="status">{deleting ? "Deleting..." : "Retry deletion to finish cleanup."}</p>
    <button className="button button-danger" disabled={deleting} onClick={() => void remove()}>Retry deletion</button>
  </>;
  if (view?.state === "missing") return <><button className="text-button" onClick={onBack}>Back to runs</button><Notice>This run is no longer available.</Notice></>;
  if (!detail)
    return (
      <>
        <button className="text-button mb-6" onClick={onBack}>
          <ArrowLeft />
          Back to runs
        </button>
        {error ? <Notice>{error}</Notice> : deleting ? <p ref={progress} tabIndex={-1} role="status">Deleting...</p> : <div className="skeleton h-64" />}
      </>
    );
  const { run } = detail;
  const active = ["running", "queued"].includes(run.status);
  const eligible = ["succeeded", "failed", "blocked", "cancelled", "interrupted"].includes(run.status);
  const current = detail.nodes.find((n) => n.id === nodeId);
  const uniqueNodeIds = new Set(visibleNodes.map(node => node.id)).size === visibleNodes.length;
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
            disabled={busy || deleting}
            onClick={() => void action(active ? "cancel" : "retry")}
          >
            {active ? <Stop size={16} /> : <ArrowClockwise size={16} />}{" "}
            {busy ? "Processing…" : active ? "Cancel" : "Retry"}
          </button>
          <button className="button button-danger" disabled={busy || deleting || !eligible}
            aria-describedby={!eligible ? "delete-run-reason" : undefined} onClick={() => setConfirm(true)}>Delete run</button>
        </div>
      </div>
      {!eligible && <p id="delete-run-reason">{active ? "Cancel this run before deleting it." : "This run cannot be deleted in its current state."}</p>}
      {deleting && <p role="status" ref={progress} tabIndex={-1}>Deleting...</p>}
      {confirm && <DeleteRunDialog run={run} busy={deleting} onCancel={() => setConfirm(false)} onConfirm={() => void remove()} />}
      {error && <Notice close={() => setError("")}>{error}</Notice>}
      {run.error && <Notice>{run.error}</Notice>}
      <div className="detail-grid">
        <section className="panel timeline">
          <div className="panel-heading">
            <h2>Workflow steps</h2>
            <span className="muted">{visibleNodes.length}</span>
          </div>
          <div className="timeline-list">
            {visibleNodes.map((node, i) => (
              <button
                key={uniqueNodeIds ? node.id : i}
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
          <RunLogOutput
            key={JSON.stringify([id, current?.id ?? ""])}
            runId={id}
            nodeId={current?.id ?? ""}
            mode={logView}
            onModeChange={setLogView}
            follow={autoScroll}
            onReadError={() => { void detailResource.current?.refresh(); }}
          />
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
