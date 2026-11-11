import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Archive, ArrowRight, FolderOpen } from "@phosphor-icons/react";
import { factoryApi, native } from "../ipc/factory-api";
import type { ImportReport } from "../../domain/types";
import { Field, Notice } from "./ui";
export function LegacyImport({
  disabled,
  onImported,
}: {
  disabled: boolean;
  onImported: () => void | Promise<void>;
}) {
  const [config, setConfig] = useState("");
  const [database, setDatabase] = useState("");
  const [error, setError] = useState("");
  const [report, setReport] = useState<ImportReport | null>(null);
  const [busy, setBusy] = useState(false);
  async function choose(kind: "config" | "database") {
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
      if (typeof path === "string")
        (kind === "config" ? setConfig : setDatabase)(path);
    } catch (e) {
      setError(String(e));
    }
  }
  async function importData() {
    setBusy(true);
    setError("");
    setReport(null);
    try {
      const result = await factoryApi.importLegacy(
        config.trim() || null,
        database.trim() || null,
      );
      setReport(result);
      await onImported();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="panel settings-section">
      <div className="section-intro">
        <div className="section-icon">
          <Archive size={20} />
        </div>
        <div>
          <h2>Import previous daemon</h2>
          <p>
            Bring over your settings and history. Original files stay untouched.
          </p>
        </div>
      </div>
      {error && <Notice>{error}</Notice>}
      {report && (
        <Notice tone="success">
          {report.runs_imported} runs imported.
          {report.settings_imported ? " Settings imported." : ""}
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
              onChange={(e) => setConfig(e.target.value)}
              disabled={busy || disabled}
            />
            <button
              className="icon-button"
              aria-label="Choose YAML config"
              disabled={!native || busy || disabled}
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
              onChange={(e) => setDatabase(e.target.value)}
              disabled={busy || disabled}
            />
            <button
              className="icon-button"
              aria-label="Choose SQLite database"
              disabled={!native || busy || disabled}
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
          className="button button-secondary"
          disabled={
            !native || busy || disabled || (!config.trim() && !database.trim())
          }
          onClick={() => void importData()}
        >
          {busy ? "Importing…" : "Import data"}
          <ArrowRight size={16} />
        </button>
      </div>
      {report && (
        <div className="import-report">
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
