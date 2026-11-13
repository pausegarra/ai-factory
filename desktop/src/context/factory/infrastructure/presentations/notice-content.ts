export function noticeDetail(reason: unknown): string {
  const value = reason instanceof Error ? reason.message : reason;
  if (typeof value !== "string") return "";
  const detail = value.trim();
  return ["", "undefined", "null", "[object Object]"].includes(detail)
    ? ""
    : detail;
}

export function noticeText(message: string, reason?: unknown): string {
  const detail = noticeDetail(reason);
  return detail && detail !== message ? `${message}\n${detail}` : message;
}

export function importSummary(runs: number, settings: boolean): string {
  return `${runs} ${runs === 1 ? "run" : "runs"} imported.${settings ? " Settings imported." : ""}`;
}
