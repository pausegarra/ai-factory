import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Archive } from "@phosphor-icons/react";
import type { ApplyBackupImport, BackupComponent, BackupInspection, PreparedBackupImport } from "../../application/dto/backup";
import { BackupRequests, backupComponents, backupLabels, backupError, backupWarning, validBackupExport, type BackupFlowState } from "../../application/services/backup-flow";
import { factoryApi, native } from "../ipc/factory-api";
import { BackupImportDialog } from "./backup-import-dialog";
import { Notice } from "./ui";

export function BackupSettings({disabled, active, onApply, onBusyChange}: {disabled: boolean; active: boolean; onApply: ApplyBackupImport; onBusyChange?: (busy: boolean) => void}) {
  const [exportComponents, setExportComponents] = useState<BackupComponent[]>(["configuration", "history"]);
  const [importComponents, setImportComponents] = useState<BackupComponent[]>([]);
  const [inspection, setInspection] = useState<BackupInspection | null>(null);
  const [path, setPath] = useState("");
  const [state, setState] = useState<BackupFlowState>({phase: "idle"});
  const [busy, setBusy] = useState(false);
  const [recovery, setRecovery] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [, changedPassword] = useState(0);
  const password = useRef<HTMLInputElement>(null); const repeat = useRef<HTMLInputElement>(null); const importPassword = useRef<HTMLInputElement>(null);
  const reviewButton = useRef<HTMLButtonElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const requests = useRef(new BackupRequests()); const mounted = useRef(false); const activeRef = useRef(active); const busyRef = useRef(false);
  const ticket = useRef<PreparedBackupImport | null>(null);
  activeRef.current = active;
  function clearPasswords() { for (const input of [password, repeat, importPassword]) if (input.current) input.current.value = ""; }
  function discardTicket() {
    const previous = ticket.current; ticket.current = null;
    if (previous) void factoryApi.discardBackupImport(previous.confirmation_id).catch(() => {});
  }
  function setWorking(value: boolean) {busyRef.current = value; setBusy(value); onBusyChange?.(value);}
  // Layout cleanup runs before React detaches the password input refs.
  useLayoutEffect(() => {
    mounted.current = true;
    return () => {mounted.current = false; requests.current.invalidate(); clearPasswords(); discardTicket(); onBusyChange?.(false);};
  }, []);
  useEffect(() => {
    requests.current.invalidate(); clearPasswords(); discardTicket(); setState({phase: "idle"}); setWorking(false);
    if (active && native) {
      const request = requests.current.begin();
      void factoryApi.backupRecoveryStatus().then(value => {
        if (mounted.current && activeRef.current && requests.current.current(request)) setRecovery(value.required);
      }).catch(error => { if (mounted.current && activeRef.current && requests.current.current(request)) setState({phase:"error",message:backupError(error)}); });
    }
  }, [active]);
  const blocked = disabled || busy || !active || !native || recovery;
  function current(request: number) {return mounted.current && activeRef.current && requests.current.current(request);}
  function changeSelection(kind: "export" | "import", component: BackupComponent) {
    if (blocked) return;
    requests.current.invalidate(); clearPasswords(); discardTicket(); setState({phase:"selecting"});
    const update = (values: BackupComponent[]) => backupComponents.filter(c => c === component ? !values.includes(c) : values.includes(c));
    (kind === "export" ? setExportComponents : setImportComponents)(update);
  }
  async function exportZip() {
    if (blocked || busyRef.current || !validBackupExport(exportComponents, password.current?.value ?? "", repeat.current?.value ?? "")) return;
    const request = requests.current.begin(); setWorking(true); setRefreshError(null);
    try {
      const destination = await save({title:"Export backup",defaultPath:"ai-factory-backup.zip",filters:[{name:"ZIP",extensions:["zip"]}]});
      if (!destination || !current(request)) return;
      const outcome = await factoryApi.exportBackup(destination,{components:exportComponents},exportComponents.includes("credentials") ? password.current?.value ?? null : null);
      if (current(request)) setState({phase:"complete",outcome});
    } catch (error) {if (current(request)) setState({phase:"error",message:backupError(error)});}
    finally {if (current(request)) {clearPasswords();setWorking(false);}}
  }
  async function chooseZip() {
    if (blocked || busyRef.current) return;
    discardTicket(); clearPasswords(); const request = requests.current.begin(); setWorking(true); setRefreshError(null); setState({phase:"inspecting"});
    try {
      const chosen = await open({title:"Choose backup ZIP",multiple:false,directory:false,filters:[{name:"ZIP",extensions:["zip"]}]});
      if (!current(request)) return;
      if (typeof chosen !== "string") {setState({phase:"idle"});return;}
      const found = await factoryApi.inspectBackup(chosen);
      if (current(request)) {setPath(chosen);setInspection(found);setImportComponents([]);setState({phase:"selecting"});}
    } catch (error) {if (current(request)) setState({phase:"error",message:backupError(error)});}
    finally {if (current(request)) setWorking(false);}
  }
  async function review() {
    if (blocked || busyRef.current || !inspection || !validBackupExport(importComponents,importPassword.current?.value ?? "",importPassword.current?.value ?? "")) return;
    discardTicket(); const request = requests.current.begin();setWorking(true);setState({phase:"preparing"});
    try {
      const prepared = await factoryApi.prepareBackupImport(path,{components:importComponents},importComponents.includes("credentials") ? importPassword.current?.value ?? null : null);
      if (!current(request)) {void factoryApi.discardBackupImport(prepared.confirmation_id).catch(() => {});return;}
      ticket.current = prepared;setState({phase:"confirming",prepared});
    } catch (error) {if (current(request)) {clearPasswords();setState({phase:"error",message:backupError(error)});}}
    finally {if (current(request)) setWorking(false);}
  }
  function cancel() { if (busyRef.current) return;discardTicket();clearPasswords();setState({phase:"selecting"}); }
  async function confirm() {
    if (busyRef.current || !ticket.current || !active || disabled) return;
    const prepared = ticket.current;ticket.current = null;const request = requests.current.begin();setWorking(true);setState({phase:"applying"});
    try {
      const result = await onApply(prepared.confirmation_id,true,importComponents.includes("credentials") ? importPassword.current?.value ?? null : null);
      if (current(request)) {setState({phase:"complete",outcome:result.outcome});setRefreshError(result.refresh_error);}
    } catch (error) {if (current(request)) {setState({phase:"error",message:backupError(error)});if (typeof error === "object" && error !== null && "code" in error && error.code === "recovery_required") setRecovery(true);}}
    finally {if (current(request)) {clearPasswords();setWorking(false);}}
  }
  async function recover() {
    if (busyRef.current || !native || !active) return;
    const request = requests.current.begin();setWorking(true);
    try {await factoryApi.recoverBackupImport();const status = await factoryApi.backupRecoveryStatus();if (current(request)) {setRecovery(status.required);setState({phase:"idle"});}}
    catch (error) {if (current(request)) setState({phase:"error",message:backupError(error)});}
    finally {if (current(request)) setWorking(false);}
  }
  const [applying, setApplying] = useState<PreparedBackupImport | null>(null);
  const dialogWasVisible = useRef(false);
  useEffect(() => {
    if (dialogWasVisible.current && !applying && activeRef.current) {
      const target = importPassword.current && !importPassword.current.disabled
        ? importPassword.current
        : reviewButton.current && !reviewButton.current.disabled ? reviewButton.current : heading.current;
      target?.focus();
    }
    dialogWasVisible.current = applying !== null;
  }, [applying]);
  // Keep the same modal mounted while its confirmed operation is in flight.
  useEffect(() => {if (state.phase === "confirming") setApplying(state.prepared);else if (state.phase !== "applying") setApplying(null);},[state]);
  return <section className="panel settings-section" aria-label="Backup and restore">
    <div className="section-intro"><div className="section-icon"><Archive size={20}/></div><div><h2 ref={heading} tabIndex={-1}>Backup and restore</h2><p>Choose the categories to transfer. Configuration and history remain visible in the ZIP. Credentials are encrypted with your backup password.</p></div></div>
    {disabled && <p>Save or discard changes before managing backups.</p>}
    {recovery && <Notice><p>Unlock the credential store and retry recovery</p><button type="button" className="button button-secondary" disabled={busy || !active} onClick={() => void recover()}>Retry recovery</button></Notice>}
    {state.phase === "error" && <Notice>{state.message}</Notice>}
    <fieldset disabled={blocked}><legend>Export categories</legend>{backupComponents.map(c => <label key={c}><input type="checkbox" checked={exportComponents.includes(c)} onChange={() => changeSelection("export",c)}/>{backupLabels[c]}</label>)}</fieldset>
    {exportComponents.includes("credentials") && <div className="form-grid"><label>Backup password<input ref={password} type="password" autoComplete="new-password" disabled={blocked} onChange={() => changedPassword(v => v + 1)}/></label><label>Repeat backup password<input ref={repeat} type="password" autoComplete="new-password" disabled={blocked} onChange={() => changedPassword(v => v + 1)}/></label><p>Use 12 to 1024 UTF8 bytes. Keep this password to import your credentials.</p></div>}
    <button type="button" className="button button-secondary" disabled={blocked || !validBackupExport(exportComponents,password.current?.value ?? "",repeat.current?.value ?? "")} onClick={() => void exportZip()}>Export ZIP</button>
    <button type="button" className="button button-secondary" disabled={blocked} onClick={() => void chooseZip()}>Choose backup ZIP</button>
    {inspection && <><p>{path.split(/[\\/]/).at(-1)}</p><fieldset disabled={blocked}><legend>Import categories</legend>{backupComponents.map(c => <label key={c}><input type="checkbox" disabled={!inspection.components.includes(c)} checked={importComponents.includes(c)} onChange={() => changeSelection("import",c)}/>{backupLabels[c]}</label>)}</fieldset>{importComponents.includes("credentials") && <label>Import password<input ref={importPassword} type="password" autoComplete="off" disabled={blocked} onChange={() => changedPassword(v => v + 1)}/></label>}<button ref={reviewButton} type="button" className="button button-secondary" disabled={blocked || !validBackupExport(importComponents,importPassword.current?.value ?? "",importPassword.current?.value ?? "")} onClick={() => void review()}>Review import</button></>}
    {busy && <p role="status">{state.phase === "applying" ? "Importing selected data…" : "Preparing backup…"}</p>}
    {state.phase === "complete" && <Notice tone="success"><p>{state.outcome.committed ? "Data imported." : "Backup exported."} {state.outcome.components.map(c => backupLabels[c]).join(", ")}</p>{state.outcome.path && <code>{state.outcome.path}</code>}{state.outcome.warnings.map((warning,index) => <p key={index}>{backupWarning(warning)}</p>)}{refreshError && <p>Could not refresh imported data. Reload the view.</p>}</Notice>}
    {applying && <BackupImportDialog prepared={applying} busy={state.phase === "applying"} onCancel={cancel} onConfirm={() => void confirm()}/>}
  </section>;
}
