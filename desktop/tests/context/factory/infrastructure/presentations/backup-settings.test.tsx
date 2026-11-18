import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { BackupComponent, BackupInspection, BackupOutcome, PreparedBackupImport } from "../../../../../src/context/factory/application/dto/backup";
vi.mock("@tauri-apps/plugin-dialog", () => ({open: vi.fn(), save: vi.fn()}));
vi.mock("../../../../../src/context/factory/infrastructure/ipc/factory-api", () => ({native: true, factoryApi: {inspectBackup: vi.fn(), prepareBackupImport: vi.fn(), discardBackupImport: vi.fn(), exportBackup: vi.fn(), backupRecoveryStatus: vi.fn(), recoverBackupImport: vi.fn()}}));
import { open, save } from "@tauri-apps/plugin-dialog";
import { factoryApi } from "../../../../../src/context/factory/infrastructure/ipc/factory-api";
import { BackupSettings } from "../../../../../src/context/factory/infrastructure/presentations/backup-settings";

const secret = "fixture password 89";
const prepared: PreparedBackupImport = {confirmation_id: "ticket-89", replace: ["history"], preserve: ["configuration", "credentials"], counts: {runs: 2}, warnings: []};
const outcome: BackupOutcome = {committed: true, components: ["history"], path: null, counts: {runs: 2}, warnings: []};
const credentialTicket: PreparedBackupImport = {...prepared, replace: ["credentials"], preserve: ["configuration", "history"]};
const credentialOutcome: BackupOutcome = {...outcome, components: ["credentials"]};
const inspection: BackupInspection = {archive_sha256: "a".repeat(64), archive_id: "fixture", components: ["configuration", "history", "credentials"], encrypted_credentials: true};
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => {resolve = done;});
  return {promise, resolve};
}
function input(name: string) {return screen.getByLabelText<HTMLInputElement>(name);}
function fill(name: string, value = secret) {fireEvent.change(input(name), {target: {value}});}
function category(group: "Export" | "Import", label: string) {
  fireEvent.click(within(screen.getByRole("group", {name: `${group} categories`})).getByLabelText(label));
}
function exportPasswords() {
  category("Export", "Credentials");
  fill("Backup password"); fill("Repeat backup password");
  return [input("Backup password"), input("Repeat backup password")];
}
async function choose() {
  fireEvent.click(screen.getByRole("button", {name: "Choose backup ZIP"}));
  await screen.findByText("history.zip");
}
async function review(component: BackupComponent = "history") {
  await choose();
  category("Import", component === "credentials" ? "Credentials" : "Database and history");
  if (component === "credentials") fill("Import password");
  fireEvent.click(screen.getByRole("button", {name: "Review import"}));
  return screen.findByRole("alertdialog");
}
afterEach(cleanup);
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(open).mockResolvedValue("/fixture/history.zip");
  vi.mocked(save).mockResolvedValue("/fixture/export.zip");
  vi.mocked(factoryApi.inspectBackup).mockResolvedValue(inspection);
  vi.mocked(factoryApi.prepareBackupImport).mockResolvedValue(prepared);
  vi.mocked(factoryApi.discardBackupImport).mockResolvedValue(undefined);
  vi.mocked(factoryApi.backupRecoveryStatus).mockResolvedValue({required: false});
  vi.mocked(factoryApi.exportBackup).mockResolvedValue({...outcome, committed: false});
});
it("does not restore until confirmed and cancellation discards the ticket", async () => {
  const apply = vi.fn(); render(<BackupSettings disabled={false} active onApply={apply}/>);
  const dialog = await review(); expect(apply).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", {name: "Cancel"}));
  expect(apply).not.toHaveBeenCalled(); expect(factoryApi.discardBackupImport).toHaveBeenCalledExactlyOnceWith("ticket-89");
});
it("applies once and keeps the committed result if refreshing fails", async () => {
  const apply = vi.fn().mockResolvedValue({outcome, refresh_error: "Could not refresh imported data"});
  render(<BackupSettings disabled={false} active onApply={apply}/>); const dialog = await review();
  const confirm = within(dialog).getByRole("button", {name: "Replace selected data"});
  fireEvent.click(confirm); fireEvent.click(confirm); await screen.findByText(/Data imported/);
  expect(apply).toHaveBeenCalledExactlyOnceWith("ticket-89", true, null);
  expect(screen.getByText(/Could not refresh imported data/)).toBeTruthy();
});
it("cancelling the destination picker does not export", async () => {
  vi.mocked(save).mockResolvedValue(null); render(<BackupSettings disabled={false} active onApply={vi.fn()}/>);
  fireEvent.click(screen.getByRole("button", {name: "Export ZIP"}));
  await vi.waitFor(() => expect(save).toHaveBeenCalled()); expect(factoryApi.exportBackup).not.toHaveBeenCalled();
});
it("returns focus to Review import after a history restore", async () => {
  render(<BackupSettings disabled={false} active onApply={vi.fn().mockResolvedValue({outcome, refresh_error: null})}/>);
  const dialog = await review(); fireEvent.click(within(dialog).getByRole("button", {name: "Replace selected data"}));
  await screen.findByText(/Data imported/);
  await vi.waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", {name: "Review import"})));
});
it("requires matching valid export passwords and passes the exact selection and password once", async () => {
  render(<BackupSettings disabled={false} active onApply={vi.fn()}/>);
  category("Export", "Credentials");
  const button = screen.getByRole<HTMLButtonElement>("button", {name: "Export ZIP"});
  for (const [first, second] of [["", ""], ["short", "short"], [secret, "different password"], ["x".repeat(1025), "x".repeat(1025)]]) {
    fill("Backup password", first); fill("Repeat backup password", second);
    expect(button.disabled).toBe(true); fireEvent.click(button);
  }
  expect(save).not.toHaveBeenCalled(); expect(factoryApi.exportBackup).not.toHaveBeenCalled();
  fill("Backup password"); fill("Repeat backup password");
  expect(button.disabled).toBe(false); fireEvent.click(button); fireEvent.click(button);
  await screen.findByText(/Backup exported/);
  expect(factoryApi.exportBackup).toHaveBeenCalledExactlyOnceWith("/fixture/export.zip", {components: ["configuration", "history", "credentials"]}, secret);
  expect(input("Backup password").value).toBe(""); expect(input("Repeat backup password").value).toBe("");
});
it.each(["cancel", "error"])("clears export passwords after picker/export %s", async result => {
  if (result === "cancel") vi.mocked(save).mockResolvedValue(null);
  else vi.mocked(factoryApi.exportBackup).mockRejectedValue({code: "persistence_failed"});
  render(<BackupSettings disabled={false} active onApply={vi.fn()}/>); const fields = exportPasswords();
  fireEvent.click(screen.getByRole("button", {name: "Export ZIP"}));
  await vi.waitFor(() => {expect(fields.map(field => field.value)).toEqual(["", ""]); expect(screen.queryByRole("status")).toBeNull();});
  if (result === "cancel") expect(factoryApi.exportBackup).not.toHaveBeenCalled();
  else expect(screen.getByText(/Could not persist/)).toBeTruthy();
});
it.each(["cancel", "success", "error", "recovery"])("clears import password and restores usable focus after %s", async result => {
  vi.mocked(factoryApi.prepareBackupImport).mockResolvedValue(credentialTicket);
  const apply = result === "error" || result === "recovery"
    ? vi.fn().mockRejectedValue({code: result === "recovery" ? "recovery_required" : "persistence_failed"})
    : vi.fn().mockResolvedValue({outcome: credentialOutcome, refresh_error: null});
  render(<BackupSettings disabled={false} active onApply={apply}/>);
  const dialog = await review("credentials"); const field = input("Import password");
  expect(factoryApi.prepareBackupImport).toHaveBeenCalledExactlyOnceWith("/fixture/history.zip", {components: ["credentials"]}, secret);
  expect(apply).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", {name: result === "cancel" ? "Cancel" : "Replace selected data"}));
  await vi.waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(field.value).toBe("");
  expect(screen.getByRole<HTMLButtonElement>("button", {name: "Review import"}).disabled).toBe(true);
  await vi.waitFor(() => expect(document.activeElement).toBe(result === "recovery" ? screen.getByRole("heading", {name: "Backup and restore"}) : field));
  if (result === "cancel") {expect(apply).not.toHaveBeenCalled(); expect(factoryApi.discardBackupImport).toHaveBeenCalledExactlyOnceWith("ticket-89");}
  else expect(apply).toHaveBeenCalledExactlyOnceWith("ticket-89", true, secret);
});
it("rejects invalid import passwords before preparing and clears the password on prepare failure", async () => {
  vi.mocked(factoryApi.prepareBackupImport).mockRejectedValue({code: "invalid_password"});
  render(<BackupSettings disabled={false} active onApply={vi.fn()}/>); await choose(); category("Import", "Credentials");
  for (const value of ["", "short", "x".repeat(1025)]) {
    fill("Import password", value);
    const button = screen.getByRole<HTMLButtonElement>("button", {name: "Review import"});
    expect(button.disabled).toBe(true); fireEvent.click(button);
  }
  expect(factoryApi.prepareBackupImport).not.toHaveBeenCalled();
  fill("Import password"); fireEvent.click(screen.getByRole("button", {name: "Review import"}));
  await screen.findByText(/Cannot unlock credentials/); expect(input("Import password").value).toBe("");
  expect(screen.queryByRole("alertdialog")).toBeNull();
});
it.each(["category", "file", "inactive", "unmount"])("clears all password inputs on %s", async transition => {
  const apply = vi.fn(); const view = render(<BackupSettings disabled={false} active onApply={apply}/>);
  await choose(); category("Import", "Credentials");
  const fields = exportPasswords(); fill("Import password"); fields.push(input("Import password"));
  if (transition === "category") category("Export", "Database and history");
  if (transition === "file") {vi.mocked(open).mockResolvedValue(null); fireEvent.click(screen.getByRole("button", {name: "Choose backup ZIP"}));}
  if (transition === "inactive") view.rerender(<BackupSettings disabled={false} active={false} onApply={apply}/>);
  if (transition === "unmount") view.unmount();
  expect(fields.map(field => field.value)).toEqual(["", "", ""]);
  expect(apply).not.toHaveBeenCalled();
});
it.each(["open", "inspect", "prepare"] as const)("ignores late %s after a new visit and keeps the current confirmation usable", async stage => {
  const oldPicker = deferred<string>(); const oldInspection = deferred<BackupInspection>(); const oldPreparation = deferred<PreparedBackupImport>();
  if (stage === "open") vi.mocked(open).mockReturnValueOnce(oldPicker.promise);
  if (stage === "inspect") vi.mocked(factoryApi.inspectBackup).mockReturnValueOnce(oldInspection.promise);
  if (stage === "prepare") vi.mocked(factoryApi.prepareBackupImport).mockReturnValueOnce(oldPreparation.promise);
  const apply = vi.fn().mockResolvedValue({outcome, refresh_error: null});
  const view = render(<BackupSettings disabled={false} active onApply={apply}/>);
  if (stage === "prepare") {
    await choose(); category("Import", "Database and history"); fireEvent.click(screen.getByRole("button", {name: "Review import"}));
    await vi.waitFor(() => expect(factoryApi.prepareBackupImport).toHaveBeenCalledTimes(1));
  } else {
    fireEvent.click(screen.getByRole("button", {name: "Choose backup ZIP"}));
    await vi.waitFor(() => expect(stage === "open" ? open : factoryApi.inspectBackup).toHaveBeenCalled());
  }
  view.rerender(<BackupSettings disabled={false} active={false} onApply={apply}/>);
  view.rerender(<BackupSettings disabled={false} active onApply={apply}/>);
  vi.mocked(open).mockResolvedValue("/fixture/current.zip");
  vi.mocked(factoryApi.prepareBackupImport).mockResolvedValue({...prepared, confirmation_id: "current-ticket", counts: {runs: 17}});
  fireEvent.click(screen.getByRole("button", {name: "Choose backup ZIP"})); await screen.findByText("current.zip");
  category("Import", "Database and history"); fireEvent.click(screen.getByRole("button", {name: "Review import"}));
  const dialog = await screen.findByRole("alertdialog"); expect(within(dialog).getByText("runs: 17")).toBeTruthy();
  await act(async () => {
    if (stage === "open") oldPicker.resolve("/fixture/obsolete.zip");
    if (stage === "inspect") oldInspection.resolve({...inspection, components: ["credentials"]});
    if (stage === "prepare") oldPreparation.resolve({...credentialTicket, confirmation_id: "obsolete-ticket"});
  });
  expect(screen.getByText("current.zip")).toBeTruthy(); expect(screen.queryByText("obsolete.zip")).toBeNull();
  expect(within(dialog).getByText("runs: 17")).toBeTruthy(); expect(apply).not.toHaveBeenCalled();
  if (stage === "prepare") expect(factoryApi.discardBackupImport).toHaveBeenCalledExactlyOnceWith("obsolete-ticket");
  else expect(factoryApi.discardBackupImport).not.toHaveBeenCalled();
  if (stage === "open") expect(factoryApi.inspectBackup).not.toHaveBeenCalledWith("/fixture/obsolete.zip");
  fireEvent.click(within(dialog).getByRole("button", {name: "Replace selected data"})); await screen.findByText(/Data imported/);
  expect(apply).toHaveBeenCalledExactlyOnceWith("current-ticket", true, null);
});
it.each(["open", "inspect", "prepare"] as const)("ignores late %s after unmount and discards obsolete prepared tickets", async stage => {
  const oldPicker = deferred<string>(); const oldInspection = deferred<BackupInspection>(); const oldPreparation = deferred<PreparedBackupImport>();
  if (stage === "open") vi.mocked(open).mockReturnValueOnce(oldPicker.promise);
  if (stage === "inspect") vi.mocked(factoryApi.inspectBackup).mockReturnValueOnce(oldInspection.promise);
  if (stage === "prepare") vi.mocked(factoryApi.prepareBackupImport).mockReturnValueOnce(oldPreparation.promise);
  const apply = vi.fn(); const view = render(<BackupSettings disabled={false} active onApply={apply}/>);
  if (stage === "prepare") {await choose(); category("Import", "Credentials"); fill("Import password"); fireEvent.click(screen.getByRole("button", {name: "Review import"}));}
  else {fireEvent.click(screen.getByRole("button", {name: "Choose backup ZIP"})); await vi.waitFor(() => expect(stage === "open" ? open : factoryApi.inspectBackup).toHaveBeenCalled());}
  view.unmount();
  await act(async () => {
    if (stage === "open") oldPicker.resolve("/fixture/obsolete.zip");
    if (stage === "inspect") oldInspection.resolve(inspection);
    if (stage === "prepare") oldPreparation.resolve({...credentialTicket, confirmation_id: "obsolete-ticket"});
  });
  expect(screen.queryByRole("alertdialog")).toBeNull(); expect(apply).not.toHaveBeenCalled();
  if (stage === "prepare") expect(factoryApi.discardBackupImport).toHaveBeenCalledExactlyOnceWith("obsolete-ticket");
  else {expect(factoryApi.prepareBackupImport).not.toHaveBeenCalled(); expect(factoryApi.discardBackupImport).not.toHaveBeenCalled();}
});
it("does not clear the current credential password when an obsolete preparation fails", async () => {
  let reject!: (error: unknown) => void;
  const old = new Promise<PreparedBackupImport>((_, fail) => {reject = fail;});
  vi.mocked(factoryApi.prepareBackupImport).mockReturnValueOnce(old);
  const apply = vi.fn().mockResolvedValue({outcome: credentialOutcome, refresh_error: null});
  const view = render(<BackupSettings disabled={false} active onApply={apply}/>);
  await choose(); category("Import", "Credentials"); fill("Import password");
  fireEvent.click(screen.getByRole("button", {name: "Review import"}));
  view.rerender(<BackupSettings disabled={false} active={false} onApply={apply}/>);
  view.rerender(<BackupSettings disabled={false} active onApply={apply}/>);
  vi.mocked(factoryApi.prepareBackupImport).mockResolvedValue({...credentialTicket, confirmation_id: "current-ticket"});
  const dialog = await review("credentials");
  await act(async () => {reject({code: "invalid_password"});});
  expect(input("Import password").value).toBe(secret);
  expect(screen.queryByText(/Cannot unlock credentials/)).toBeNull();
  fireEvent.click(within(dialog).getByRole("button", {name: "Replace selected data"}));
  await screen.findByText(/Data imported/);
  expect(apply).toHaveBeenCalledExactlyOnceWith("current-ticket", true, secret);
});
it("does not clear new export passwords when an obsolete export finishes", async () => {
  const old = deferred<BackupOutcome>(); vi.mocked(factoryApi.exportBackup).mockReturnValueOnce(old.promise);
  const view = render(<BackupSettings disabled={false} active onApply={vi.fn()}/>);
  exportPasswords(); fireEvent.click(screen.getByRole("button", {name: "Export ZIP"}));
  await vi.waitFor(() => expect(factoryApi.exportBackup).toHaveBeenCalledTimes(1));
  view.rerender(<BackupSettings disabled={false} active={false} onApply={vi.fn()}/>);
  view.rerender(<BackupSettings disabled={false} active onApply={vi.fn()}/>);
  fill("Backup password"); fill("Repeat backup password");
  await act(async () => {old.resolve({...credentialOutcome, committed: false});});
  expect(input("Backup password").value).toBe(secret); expect(input("Repeat backup password").value).toBe(secret);
  expect(screen.queryByText(/Backup exported/)).toBeNull();
});
it.each(["success", "error"])("does not clear a new confirmation password when an obsolete apply returns %s", async result => {
  let resolve!: (value: {outcome: BackupOutcome; refresh_error: null}) => void;
  let reject!: (error: unknown) => void;
  const old = new Promise<{outcome: BackupOutcome; refresh_error: null}>((done, fail) => {resolve = done; reject = fail;});
  const apply = vi.fn().mockReturnValueOnce(old).mockResolvedValue({outcome: credentialOutcome, refresh_error: null});
  vi.mocked(factoryApi.prepareBackupImport).mockResolvedValue(credentialTicket);
  const view = render(<BackupSettings disabled={false} active onApply={apply}/>);
  const first = await review("credentials"); fireEvent.click(within(first).getByRole("button", {name: "Replace selected data"}));
  view.rerender(<BackupSettings disabled={false} active={false} onApply={apply}/>);
  view.rerender(<BackupSettings disabled={false} active onApply={apply}/>);
  vi.mocked(factoryApi.prepareBackupImport).mockResolvedValue({...credentialTicket, confirmation_id: "current-ticket"});
  const current = await review("credentials");
  await act(async () => {if (result === "success") resolve({outcome: credentialOutcome, refresh_error: null}); else reject({code: "persistence_failed"});});
  expect(input("Import password").value).toBe(secret);
  expect(screen.getByRole("alertdialog")).toBe(current);
  expect(screen.queryByText(/Data imported|Could not persist/)).toBeNull();
  fireEvent.click(within(current).getByRole("button", {name: "Replace selected data"})); await screen.findByText(/Data imported/);
  expect(apply).toHaveBeenNthCalledWith(2, "current-ticket", true, secret);
});
