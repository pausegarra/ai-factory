import { expect, it } from "vitest";
import { validBackupExport, BackupRequests, backupError } from "../../../../src/context/factory/application/services/backup-flow";
it("requires explicit selection and matching passwords measured as UTF8 bytes", () => {
 expect(validBackupExport([],"","")).toBe(false);
 expect(validBackupExport(["history"],"","")).toBe(true);
 expect(validBackupExport(["credentials"],"short","short")).toBe(false);
 expect(validBackupExport(["credentials"],"fixture password 89","mismatch")).toBe(false);
 expect(validBackupExport(["credentials"],"á".repeat(6),"á".repeat(6))).toBe(true);
 expect(validBackupExport(["credentials"],"á".repeat(513),"á".repeat(513))).toBe(false);
});
it("invalidates asynchronous results when a file or visit changes", () => {
 const requests=new BackupRequests();const first=requests.begin();expect(requests.current(first)).toBe(true);requests.invalidate();expect(requests.current(first)).toBe(false);const second=requests.begin();expect(requests.current(second)).toBe(true);expect(requests.current(first)).toBe(false);
});
it("never displays raw archive errors or secrets", () => {
 expect(backupError({code:"invalid_archive",message:"fixture-secret"})).not.toContain("fixture-secret");
 expect(backupError(new Error("fixture-secret"))).not.toContain("fixture-secret");
});
