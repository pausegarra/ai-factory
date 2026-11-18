import { useEffect, useId, useRef } from "react";
import type { PreparedBackupImport } from "../../application/dto/backup";
import { backupLabels, backupWarning } from "../../application/services/backup-flow";
export function BackupImportDialog({prepared, busy, onCancel, onConfirm}: {prepared: PreparedBackupImport; busy: boolean; onCancel: () => void; onConfirm: () => void}) {
  const title = useId(); const description = useId(); const cancel = useRef<HTMLButtonElement>(null); const panel = useRef<HTMLDivElement>(null);
  useEffect(() => { cancel.current?.focus(); }, []);
  return <div className="dialog-backdrop"><div ref={panel} className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby={title} aria-describedby={description} onKeyDown={(event) => {
    if (event.key === "Escape" && !busy) { event.preventDefault(); onCancel(); }
    if (event.key === "Tab") {
      const buttons = Array.from(panel.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
      const first = buttons[0]; const last = buttons.at(-1);
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && document.activeElement === first) {event.preventDefault(); last?.focus();}
      if (!event.shiftKey && document.activeElement === last) {event.preventDefault(); first.focus();}
    }
  }}>
    <h2 id={title}>Import selected data?</h2>
    <p id={description}>This replaces the selected categories in this installation. Review the replacement before continuing.</p>
    <p>Will replace: {prepared.replace.map(c => backupLabels[c]).join(", ")}</p>
    <p>Will keep: {prepared.preserve.map(c => backupLabels[c]).join(", ") || "None"}</p>
    <ul>{Object.entries(prepared.counts).map(([key,value]) => <li key={key}>{key}: {value}</li>)}{prepared.warnings.map((warning,index) => <li key={index}>{backupWarning(warning)}</li>)}</ul>
    <div className="dialog-actions"><button ref={cancel} type="button" className="button button-secondary" disabled={busy} onClick={onCancel}>Cancel</button><button type="button" className="button" disabled={busy} onClick={onConfirm}>{busy ? "Importing…" : "Replace selected data"}</button></div>
  </div></div>;
}
