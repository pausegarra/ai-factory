import { DEFAULT_ISSUE_TITLE_PREFIX, DEFAULT_RELEASE_ISSUE_TITLE_PREFIX, type SourceConnector } from "./types";
export type SourceScopeField = "owners" | "authors" | "reviewers" | "pull_request_authors";
export function newGitHubIssuesSource(id: string): SourceConnector {
  return {id, label:"GitHub Issues", type:"github_issues", account:"", owners:[], authors:[], reviewers:[], has_token:false,
    issue_title_prefix: DEFAULT_ISSUE_TITLE_PREFIX, release_issue_title_prefix: DEFAULT_RELEASE_ISSUE_TITLE_PREFIX,
    review_all_open_pull_requests:false, pull_request_authors:[]};
}
export function cleanSourceConnectors(items: SourceConnector[]): SourceConnector[] {
  return structuredClone(items).map(source => {
    delete source.token;
    if (source.pull_request_authors === undefined) source.pull_request_authors = [];
    if (source.review_all_open_pull_requests === undefined) source.review_all_open_pull_requests = false;
    return source;
  });
}
export function parseSourceScope(value: string): string[] {
  return value.split(",").map(item => item.trim()).filter(Boolean);
}
export function normalizePullRequestAuthors(values: readonly string[]): string[] {
  return [...new Set(values.map(value => value.trim().toLowerCase()).filter(Boolean))];
}
export function isInvalidSourceTitlePrefix(value: unknown): boolean {
  return value !== undefined && (typeof value !== "string" || !value.trim());
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
    if (isInvalidSourceTitlePrefix(source.issue_title_prefix)) errors.push("Issue title prefix cannot be empty.");
    if (isInvalidSourceTitlePrefix(source.release_issue_title_prefix)) errors.push("Release issue title prefix cannot be empty.");
    for (const field of ["owners", "authors", "reviewers"] as const) {
      const values = source[field];
      if (!Array.isArray(values) || values.some(value => typeof value !== "string" || !/^[a-zA-Z0-9_.\[\]-]{1,100}$/.test(value))) errors.push(`${field} must contain valid GitHub logins.`);
    }
    const prAuthors = source.pull_request_authors;
    if (prAuthors !== undefined && (!Array.isArray(prAuthors) || prAuthors.some(
      value => typeof value !== "string" || !/^[a-zA-Z0-9_.\[\]-]{1,100}$/.test(value)
    ))) errors.push("pull_request_authors must contain valid GitHub logins.");
    if (!source.has_token && !source.token?.trim()) errors.push("Source connector token is required.");
  }
  return errors;
}

export function sourceLabel(source: SourceConnector, sources: SourceConnector[]): string {
  const label = `${source.label || source.account} (@${source.account})`;
  return label + (sources.some(other => other.id !== source.id && other.label === source.label && other.account === source.account) ? ` (${source.id})` : "");
}
