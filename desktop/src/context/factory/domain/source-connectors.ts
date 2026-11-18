import type { SourceConnector } from "./types";
export type SourceScopeField = "owners" | "authors" | "reviewers";
export function newGitHubIssuesSource(id: string): SourceConnector {
  return {id, label:"GitHub Issues", type:"github_issues", account:"", owners:[], authors:[], reviewers:[], has_token:false, review_all_open_pull_requests:false};
}
export function cleanSourceConnectors(items: SourceConnector[]): SourceConnector[] {
  return structuredClone(items).map(source => {
    delete source.token;
    if (source.review_all_open_pull_requests === undefined) source.review_all_open_pull_requests = false;
    return source;
  });
}
export function parseSourceScope(value: string): string[] {
  return value.split(",").map(item => item.trim()).filter(Boolean);
}
export function validateSourceConnectors(items: SourceConnector[]): string[] {
  const errors: string[] = [], ids = new Set<string>();
  for (const source of items) {
    if ((!/^[A-Za-z0-9_.-]{1,128}$/.test(source.id) || source.id === "." || source.id === "..") || ids.has(source.id)) errors.push("Source connector IDs must be valid and unique.");
    ids.add(source.id);
    if (source.type !== "github_issues") errors.push("Unsupported source connector type.");
    if (source.review_all_open_pull_requests !== undefined && typeof source.review_all_open_pull_requests !== "boolean") errors.push("PR review scope must be a boolean.");
    if (!source.label.trim()) errors.push("Source connector name is required.");
    if (!/^[a-zA-Z0-9_.\[\]-]{1,100}$/.test(source.account)) errors.push("Enter a valid GitHub account.");
    for (const field of ["owners", "authors", "reviewers"] as const) {
      const values = source[field];
      if (!Array.isArray(values) || values.some(value => typeof value !== "string" || !/^[a-zA-Z0-9_.\[\]-]{1,100}$/.test(value))) errors.push(`${field} must contain valid GitHub logins.`);
    }
    if (!source.has_token && !source.token?.trim()) errors.push("Source connector token is required.");
  }
  return errors;
}

export function sourceLabel(source: SourceConnector, sources: SourceConnector[]): string {
  const label = `${source.label || source.account} (@${source.account})`;
  return label + (sources.some(other => other.id !== source.id && other.label === source.label && other.account === source.account) ? ` (${source.id})` : "");
}
