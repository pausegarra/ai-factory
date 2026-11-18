import type { SourceConnector } from "../src/context/factory/domain/types";
import { expect, it } from "vitest";
import { newGitHubIssuesSource, cleanSourceConnectors, validateSourceConnectors,
  parseSourceScope } from "../src/context/factory/domain/source-connectors";

it("validates independent sources and strips secrets without mutating drafts", () => {
  const a = {...newGitHubIssuesSource("src-a"), label: "Work", account: "alice", token: "fixture-a"};
  const b = {...newGitHubIssuesSource("src-b"), label: "Personal", account: "alice", token: "fixture-b"};
  expect(validateSourceConnectors([a, b])).toEqual([]);
  expect(validateSourceConnectors([a, {...b, id: a.id}]).join(" ")).toMatch(/unique/i);
  expect(cleanSourceConnectors([a])[0].token).toBeUndefined();
  expect(a.token).toBe("fixture-a");
  expect(parseSourceScope(" team, , second ")).toEqual(["team", "second"]);
});

it("defaults the review scope and preserves both values without leaking tokens", () => {
  expect(newGitHubIssuesSource("fresh").review_all_open_pull_requests).toBe(false);
  for (const value of [false, true]) {
    const source = {...newGitHubIssuesSource("work"), account:"alice",
      has_token:true, review_all_open_pull_requests:value, token:"fixture-secret"};
    const cleaned = cleanSourceConnectors([source])[0];
    expect(cleaned.review_all_open_pull_requests).toBe(value);
    expect(cleaned.token).toBeUndefined();
    expect(source.token).toBe("fixture-secret");
    expect(validateSourceConnectors([cleaned])).toEqual([]);
  }
  const legacy: SourceConnector = {id:"legacy", label:"Legacy", type:"github_issues",
    account:"alice", has_token:true, owners:["acme"], authors:[], reviewers:["alice"]};
  expect(validateSourceConnectors([legacy])).toEqual([]);
  expect(cleanSourceConnectors([legacy])[0].review_all_open_pull_requests).toBe(false);
  expect("review_all_open_pull_requests" in legacy).toBe(false);
});

it("rejects a present non boolean review scope", () => {
  for (const value of ["true", 1, null]) {
    const invalid = {...newGitHubIssuesSource("work"), account:"alice",
      has_token:true, review_all_open_pull_requests:value} as unknown as SourceConnector;
    expect(validateSourceConnectors([invalid]).join(" ")).toContain(
      "PR review scope must be a boolean."
    );
  }
});
