import type { BackupComponent, BackupOutcome, PreparedBackupImport, BackupWarning } from "../dto/backup";
export const backupComponents: BackupComponent[] = ["configuration", "history", "credentials"];
export const backupLabels: Record<BackupComponent, string> = { configuration: "Configuration", history: "Database and history", credentials: "Credentials" };
export type BackupFlowState = {phase: "idle" | "inspecting" | "selecting" | "preparing" | "applying"} | {phase: "confirming"; prepared: PreparedBackupImport} | {phase: "complete"; outcome: BackupOutcome} | {phase: "error"; message: string};
export class BackupRequests {
  private generation = 0;
  begin() { return ++this.generation; }
  current(request: number) { return request === this.generation; }
  invalidate() { this.generation += 1; }
}
export function validBackupExport(components: BackupComponent[], password: string, repeat: string) {
  if (!components.length || new Set(components).size !== components.length) return false;
  if (!components.includes("credentials")) return true;
  const length = new TextEncoder().encode(password).length;
  return length >= 12 && length <= 1024 && password === repeat;
}
export function backupError(error: unknown): string {
  const code = typeof error === "object" && error !== null && "code" in error ? String(error.code) : "unknown";
  const messages: Record<string,string> = {
    invalid_selection: "Choose at least one available category.", invalid_archive: "This file is not a valid AI Factory backup.", unsupported_version: "This backup format version is not supported.", limit_exceeded: "This backup exceeds supported limits.", invalid_password: "Cannot unlock credentials. Check the backup password.", credential_unavailable: "Unlock the credential store and retry.", credential_target_missing: "Import the matching configuration with these credentials.", operation_busy: "Wait for active work or cancel it, then retry.", cleanup_busy: "Finish pending run deletion before importing history.", stale_preview: "Backup or local data changed. Review the import again.", confirmation_required: "Review and confirm the replacement before importing.", persistence_failed: "Could not persist backup data. Retry the operation.", destination_exists: "Choose a new destination filename.", recovery_required: "Unlock the credential store and retry recovery.", desktop_required: "Open AI Factory desktop to manage backups.",
  };
  return messages[code] ?? "Could not complete the backup operation.";
}
export function backupWarning(warning: BackupWarning): string {
  const messages: Record<string,string> = {credential_missing:"Some credentials are unavailable. Enter them again in Settings.",credential_omitted:"Credentials were omitted.",codex_binding_missing:"An agent account assignment was omitted. Choose a local account in Settings.",external_files_omitted:"Workspaces, sessions, logs and external files are not included.",redaction_applied:"Managed secrets were removed from public data.",local_startup_preserved:"The local startup preference was preserved.",refresh_failed:"Data was imported. Refresh the view to load it."};
  return messages[warning.code] ?? "Review imported settings before running workflows.";
}
