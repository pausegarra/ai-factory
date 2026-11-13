import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Archive, ArrowRight, FolderOpen } from "@phosphor-icons/react";
import { factoryApi, native } from "../ipc/factory-api";
import type { ImportReport } from "../../domain/types";
import { importSummary, noticeText } from "./notice-content";
import { Field, Notice } from "./ui";

type ImportBanner = { kind: "success" | "error"; message: string };

export function LegacyImport({
  disabled,
  onImported,
  active = true,
}: {
  disabled: boolean;
  onImported: () => void | Promise<void>;
  active?: boolean;
}) {
  const [config, setConfig] = useState("");
  const [database, setDatabase] = useState("");
  const [banner, setBanner] = useState<ImportBanner | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [busy, setBusy] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const mounted = useRef(false);
  const activeRef = useRef(active);
  const previousActive = useRef(active);
  const visit = useRef(0);
  const attempt = useRef(0);
  const picker = useRef(0);
  const busyRef = useRef(false);
  activeRef.current = active;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      visit.current += 1;
      attempt.current += 1;
      picker.current += 1;
    };
  }, []);
  useEffect(() => {
    if (previousActive.current !== active) {
      previousActive.current = active;
      visit.current += 1;
      picker.current += 1;
      setBanner(null);
    }
  }, [active]);

  function changeInput(setValue: (value: string) => void, value: string) {
    picker.current += 1;
    setBanner(null);
    setValue(value);
  }

  async function choose(kind: "config" | "database") {
    const pickerTicket = ++picker.current;
    const visitTicket = visit.current;
    setBanner(null);
    try {
      const path = await open({
        multiple: false,
        directory: false,
        title:
          kind === "config"
            ? "Choose config.yaml"
            : "Choose previous SQLite database",
        filters:
          kind === "config"
            ? [{ name: "YAML", extensions: ["yaml", "yml"] }]
            : [{ name: "SQLite", extensions: ["db", "sqlite", "sqlite3"] }],
      });
      if (
        typeof path === "string" &&
        mounted.current &&
        activeRef.current &&
        visit.current === visitTicket &&
        picker.current === pickerTicket
      ) {
        (kind === "config" ? setConfig : setDatabase)(path);
      }
    } catch (reason) {
      if (
        mounted.current && activeRef.current && visit.current === visitTicket &&
        picker.current === pickerTicket
      ) {
        setBanner({
          kind: "error",
          message: noticeText("Could not choose import file.", reason),
        });
      }
    }
  }

  async function importData() {
    if (busyRef.current || disabled || !active) return;
    busyRef.current = true;
    setBusy(true);
    setBanner(null);
    setReport(null);
    const attemptTicket = ++attempt.current;
    const visitTicket = visit.current;
    const currentAttempt = () => mounted.current && attempt.current === attemptTicket;
    const visibleVisit = () => activeRef.current && visit.current === visitTicket;
    try {
      let result: ImportReport;
      try {
        result = await factoryApi.importLegacy(
          config.trim() || null,
          database.trim() || null,
        );
      } catch (reason) {
        if (currentAttempt() && visibleVisit()) {
          setBanner({ kind: "error", message: noticeText("Could not import data.", reason) });
        }
        return;
      }
      if (!currentAttempt()) return;
      setReport(result);
      try {
        await onImported();
        if (currentAttempt() && visibleVisit()) {
          setBanner({ kind: "success", message: importSummary(result.runs_imported, result.settings_imported) });
        }
      } catch (reason) {
        if (currentAttempt() && visibleVisit()) {
          setBanner({
            kind: "error",
            message: noticeText("Data imported, but settings could not be refreshed.", reason),
          });
        }
      }
    } finally {
      if (currentAttempt()) {
        busyRef.current = false;
        setBusy(false);
      }
    }
  }

  return (
    <section className="panel settings-section">
      <div className="section-intro">
        <div className="section-icon">
          <Archive size={20} />
        </div>
        <div>
          <h2 ref={heading} tabIndex={-1}>
            Import previous daemon
          </h2>
          <p>
            Bring over your settings and history. Original files stay untouched.
          </p>
        </div>
      </div>
      {active && banner && (
        <Notice
          tone={banner.kind === "success" ? "success" : "error"}
          close={() => setBanner(null)}
          focusAfterClose={() => heading.current}
        >
          {banner.message}
        </Notice>
      )}
      <div className="form-grid">
        <Field
          label="Previous YAML config"
          hint="Optional. Replaces current settings."
        >
          <div className="input-with-button">
            <input
              aria-label="Previous YAML config"
              placeholder="/path/config.yaml"
              value={config}
              onChange={(e) => changeInput(setConfig, e.target.value)}
              disabled={busy || disabled || !active}
            />
            <button
              type="button"
              className="icon-button"
              aria-label="Choose YAML config"
              disabled={!native || busy || disabled || !active}
              onClick={() => void choose("config")}
            >
              <FolderOpen size={18} />
            </button>
          </div>
        </Field>
        <Field
          label="Previous SQLite database"
          hint="Optional. Adds history to this app."
        >
          <div className="input-with-button">
            <input
              aria-label="Previous SQLite database"
              placeholder="/path/state.db"
              value={database}
              onChange={(e) => changeInput(setDatabase, e.target.value)}
              disabled={busy || disabled || !active}
            />
            <button
              type="button"
              className="icon-button"
              aria-label="Choose SQLite database"
              disabled={!native || busy || disabled || !active}
              onClick={() => void choose("database")}
            >
              <FolderOpen size={18} />
            </button>
          </div>
        </Field>
      </div>
      <div className="import-actions">
        <span>
          {disabled
            ? "Save or discard changes before importing."
            : "Choose at least one file to import."}
        </span>
        <button
          type="button"
          className="button button-secondary"
          disabled={
            !native || busy || disabled || !active || (!config.trim() && !database.trim())
          }
          onClick={() => void importData()}
        >
          {busy ? "Importing…" : "Import data"}
          <ArrowRight size={16} />
        </button>
      </div>
      {report && (
        <div className="import-report" aria-label="Import report">
          <span>{importSummary(report.runs_imported, report.settings_imported)}</span>
          <span>Archived copy</span>
          <code>{report.archive_path}</code>
          {report.warnings.length > 0 && (
            <ul>
              {report.warnings.map((warning, i) => (
                <li key={i}>{warning}</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
