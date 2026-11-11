import { useEffect, useState } from "react";
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
  const [selectedRun, setSelectedRun] = useState<string | null>(null);
  const [hasUnsaved, setHasUnsaved] = useState(false);
  const [pendingPage, setPendingPage] = useState<string | null>(null);
  const [polling, setPolling] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const live = new LiveResource(
      {
        listen: (cb) =>
          factoryApi.subscribe((e) => {
            if (e.kind !== "log") cb();
          }),
        load: async () => {
          const [settings, snapshot] = await Promise.all([
            factoryApi.settings(),
            factoryApi.snapshot(),
          ]);
          return { settings, snapshot };
        },
      },
      (data) => {
        setSettings(data.settings);
        setSnapshot(data.snapshot);
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
  }
  function navigate(value: string) {
    if (value === page) return;
    if (hasUnsaved) {
      setPendingPage(value);
      return;
    }
    setPage(value);
    setSelectedRun(null);
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
          ) : selectedRun ? (
            <RunDetailPage
              key={selectedRun}
              id={selectedRun}
              onBack={() => setSelectedRun(null)}
              onSelect={setSelectedRun}
            />
          ) : page === "overview" ? (
            <OverviewPage
              settings={settings}
              snapshot={snapshot}
              onPage={navigate}
              onRun={(id) => {
                setPage("runs");
                setSelectedRun(id);
              }}
            />
          ) : page === "runs" ? (
            <RunsPage snapshot={snapshot} onSelect={setSelectedRun} />
          ) : page === "flows" ? (
            <FlowsPage
              settings={settings}
              onSave={saveSettings}
              onDirtyChange={setHasUnsaved}
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
            <div>
              <button
                autoFocus
                className="button button-secondary"
                onClick={() => setPendingPage(null)}
              >
                Keep editing
              </button>
              <button
                className="button button-primary"
                onClick={() => {
                  setPage(pendingPage);
                  setSelectedRun(null);
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
