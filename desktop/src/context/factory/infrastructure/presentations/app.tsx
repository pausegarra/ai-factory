import { useEffect, useRef, useState } from "react";
import {
  ArrowClockwise,
  ArrowUpRight,
  ChartBar,
  Factory,
  Gear,
  GitBranch,
  Lightning,
  SidebarSimple,
  Terminal,
  WifiHigh,
} from "@phosphor-icons/react";
import logo from "../../../../assets/generated/app-icon.png";
import type { Settings, Snapshot } from "../../domain/types";
import { factoryApi, native } from "../ipc/factory-api";
import { LiveResource } from "../../application/live-resource";
import { OverviewPage } from "./overview";
import { RunsPage, RunDetailPage } from "./runs";
import { FlowsPage } from "./flows";
import { SettingsPage } from "./settings";
import { Notice } from "./ui";
type RunSelection = { id: string; revision: number };
type RunFeedback = {
  tone: "success" | "info";
  message: "Run deleted." | "This run is no longer available.";
};
const nav = [
  { id: "overview", label: "Overview", icon: ChartBar },
  { id: "runs", label: "Runs", icon: Lightning },
  { id: "flows", label: "Workflows", icon: GitBranch },
  { id: "settings", label: "Settings", icon: Gear },
];
export function App() {
  const [page, setPage] = useState("overview");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [error, setError] = useState("");
  const [selection, setSelection] = useState<RunSelection | null>(null);
  const selected = useRef<RunSelection | null>(null);
  const sequence = useRef(0);
  const initiated = useRef(new Set<RunSelection>());
  const [notice, setNotice] = useState<RunFeedback | null>(null);
  const runsHeading = useRef<HTMLHeadingElement>(null);
  function dismissRunNotice() {
    setNotice(null);
    runsHeading.current?.focus();
  }
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  function commitContext(value: string, next: RunSelection | null = null) {
    selected.current = next;
    initiated.current.clear();
    setSelection(next);
    setPage(value);
    setNotice(null);
  }
  function selectRun(id: string) {
    commitContext("runs", { id, revision: ++sequence.current });
  }
  function unavailable(
    id: string,
    deleted = false,
    expected: RunSelection | null = selected.current,
  ) {
    if (!alive.current || !expected || selected.current !== expected || expected.id !== id) return;
    const local = deleted || initiated.current.has(expected);
    commitContext("runs");
    setNotice(local
      ? { tone: "success", message: "Run deleted." }
      : { tone: "info", message: "This run is no longer available." });
  }
  function refreshAfterDeletion() {
    if (alive.current) setReload(value => value + 1);
  }
  const [hasUnsaved, setHasUnsaved] = useState(false);
  const [savingWorkflow, setSavingWorkflow] = useState(false);
  const [pendingPage, setPendingPage] = useState<string | null>(null);
  const [polling, setPolling] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const live = new LiveResource(
      {
        listen: (cb) =>
          factoryApi.subscribe((e) => {
            if (e.kind === "run_deleted" && e.run_id) unavailable(e.run_id);
            if (e.kind !== "log") cb();
          }),
        load: async () => {
          const selection = selected.current;
          const [settings, snapshot] = await Promise.all([
            factoryApi.settings(),
            factoryApi.snapshot(),
          ]);
          return { settings, snapshot, selection };
        },
      },
      (data) => {
        setSettings(data.settings);
        setSnapshot(data.snapshot);
        if (data.selection && selected.current === data.selection &&
          !data.snapshot.runs.some(run => run.id === data.selection!.id) &&
          !data.snapshot.deletions.some(item => item.run_id === data.selection!.id)) {
          unavailable(data.selection.id, false, data.selection);
        }
        setError("");
      },
      (e) => setError(String(e)),
    );
    void live.start();
    return () => live.stop();
  }, [reload]);
  async function saveSettings(value: Settings) {
    const result = await factoryApi.saveSettings(value);
    setSettings(result);
    return result;
  }
  function navigate(value: string) {
    if (savingWorkflow) return;
    if (value === page) return;
    if (hasUnsaved) {
      setPendingPage(value);
      return;
    }
    commitContext(value);
  }
  async function poll() {
    setPolling(true);
    setError("");
    try {
      await factoryApi.poll();
    } catch (e) {
      setError(String(e));
    } finally {
      setPolling(false);
    }
  }
  const totalActive = snapshot
    ? snapshot.metrics.running + snapshot.metrics.queued
    : 0;
  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        Skip to content
      </a>
      <aside className="sidebar">
        <div className="brand">
          <img src={logo} alt="" />
          <div>
            <strong>AI Factory</strong>
            <small>Ideas into production</small>
          </div>
        </div>
        <span className="nav-caption">WORKSPACE</span>
        <nav aria-label="Main navigation">
          {nav.map((item) => (
            <button
              key={item.id}
              disabled={savingWorkflow}
              aria-current={page === item.id ? "page" : undefined}
              className={`nav-item ${page === item.id ? "active" : ""}`}
              onClick={() => navigate(item.id)}
            >
              <item.icon
                size={20}
                weight={page === item.id ? "fill" : "regular"}
              />
              <span>{item.label}</span>
              {item.id === "runs" && totalActive > 0 && (
                <span className="nav-count">{totalActive}</span>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="engine-status">
            <span
              className={snapshot?.service.running ? "online" : "offline"}
            />
            <div>
              <strong>
                {native
                  ? snapshot?.service.running
                    ? "Engine running"
                    : snapshot
                      ? "Engine stopped"
                      : "Engine starting"
                  : "Preview"}
              </strong>
              <small>
                {native
                  ? "Everything on your device"
                  : "Open the app to connect"}
              </small>
            </div>
          </div>
          <div className="sidebar-footer">
            <span>LOCAL FIRST</span>
            <span>v0.2.0</span>
          </div>
        </div>
      </aside>
      <div className="workspace">
        <header className="topbar">
          <div className="breadcrumb">
            <Factory size={16} />
            <span>Factory</span>
            <span className="breadcrumb-slash">/</span>
            <strong>{nav.find((n) => n.id === page)?.label}</strong>
          </div>
          <div className="topbar-actions">
            <span className="connection-label">
              <span />
              {native ? "Local connection" : "Desktop preview"}
            </span>
            <button
              className="button button-topbar"
              disabled={!native || polling || !!snapshot?.service.polling}
              onClick={() => void poll()}
            >
              <ArrowClockwise
                size={15}
                className={
                  polling || snapshot?.service.polling ? "rotating" : ""
                }
              />
              {polling || snapshot?.service.polling ? "Syncing…" : "Sync now"}
            </button>
          </div>
        </header>
        <main
          id="main-content"
          className={`main-content ${page === "flows" ? "flow-page" : ""}`}
        >
          {!native && (
            <div className="preview-notice">
              <Terminal size={16} />
              <span>
                Interface preview. Live data and runs are available in the
                desktop app.
              </span>
              <span className="preview-tag">PREVIEW</span>
            </div>
          )}
          {error && (
            <Notice close={() => setError("")}>
              {error}
              <button
                className="text-button"
                onClick={() => setReload((x) => x + 1)}
              >
                Retry
              </button>
            </Notice>
          )}
          {snapshot?.service.last_error && (
            <Notice>GitHub sync failed: {snapshot.service.last_error}</Notice>
          )}
          {!settings || !snapshot ? (
            <div className="loading-view">
              <div className="skeleton h-8 w-64" />
              <div className="skeleton h-4 w-96" />
              <div className="metrics-grid mt-8">
                {[1, 2, 3, 4].map((i) => (
                  <div className="skeleton h-40" key={i} />
                ))}
              </div>
              <div className="skeleton h-96 mt-6" />
            </div>
          ) : selection ? (
            <RunDetailPage
              key={`${selection.id}:${selection.revision}`}
              id={selection.id}
              onBack={() => commitContext("runs")}
              onSelect={selectRun}
              onDeletionStarted={id => {
                if (alive.current && selected.current === selection && selection.id === id) {
                  initiated.current.add(selection);
                }
              }}
              onDeletionSettled={() => {
                initiated.current.delete(selection);
                refreshAfterDeletion();
              }}
              onDeleted={id => {
                unavailable(id, true, selection);
                refreshAfterDeletion();
              }}
              onUnavailable={id => {
                unavailable(id, false, selection);
                refreshAfterDeletion();
              }}
            />
          ) : page === "overview" ? (
            <OverviewPage
              settings={settings}
              snapshot={snapshot}
              onPage={navigate}
              onRun={selectRun}
            />
          ) : page === "runs" ? (
            <>
              {notice && (
                <Notice tone={notice.tone} close={dismissRunNotice}>
                  {notice.message}
                </Notice>
              )}
              <RunsPage snapshot={snapshot} onSelect={selectRun} headingRef={runsHeading} />
            </>
          ) : page === "flows" ? (
            <FlowsPage
              settings={settings}
              onSave={saveSettings}
              onLoadSkills={factoryApi.installedSkills}
              onDirtyChange={setHasUnsaved}
              onBusyChange={setSavingWorkflow}
            />
          ) : (
            <SettingsPage
              key={settings.token_profiles.map((t) => t.id).join(",")}
              settings={settings}
              onSave={saveSettings}
              appDir={snapshot.service.app_data_dir}
              onDirtyChange={setHasUnsaved}
            />
          )}
        </main>
      </div>
      {pendingPage && (
        <div
          className="dialog-backdrop"
          onKeyDown={(event) => {
            if (event.key === "Escape") setPendingPage(null);
            if (event.key === "Tab") {
              const buttons =
                event.currentTarget.querySelectorAll<HTMLButtonElement>(
                  "button",
                );
              const first = buttons[0],
                last = buttons[buttons.length - 1];
              if (event.shiftKey && document.activeElement === first) {
                event.preventDefault();
                last.focus();
              } else if (!event.shiftKey && document.activeElement === last) {
                event.preventDefault();
                first.focus();
              }
            }
          }}
        >
          <section
            role="alertdialog"
            aria-labelledby="unsaved-heading"
            aria-describedby="unsaved-description"
            aria-modal="true"
            className="confirm-dialog"
          >
            <span className="eyebrow">UNSAVED CHANGES</span>
            <h2 id="unsaved-heading">Your work is still here</h2>
            <p id="unsaved-description">
              Save your changes before leaving, or discard them to open another
              page.
            </p>
            <div className="dialog-actions">
              <button
                autoFocus
                className="button button-secondary"
                onClick={() => setPendingPage(null)}
              >
                Keep editing
              </button>
              <button
                className="button button-primary"
                disabled={savingWorkflow}
                onClick={() => {
                  if (savingWorkflow) return;
                  commitContext(pendingPage);
                  setHasUnsaved(false);
                  setPendingPage(null);
                }}
              >
                Discard changes
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
