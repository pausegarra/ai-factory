import type { SourceConnector } from "../src/context/factory/domain/types";
import { expect, it } from "vitest";
import { newGitHubIssuesSource, cleanSourceConnectors, validateSourceConnectors,
  parseSourceScope, normalizePullRequestAuthors } from "../src/context/factory/domain/source-connectors";

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

it("defaults, normalizes and isolates PR authors without changing issue authors", () => {
  const fresh = newGitHubIssuesSource("fresh");
  expect(fresh.pull_request_authors).toEqual([]);
  const raw = [" CCarneroMerida ", "", "  ", "ccarneromerida", "NEXT-dvalverde", " laubernal "];
  expect(normalizePullRequestAuthors(raw)).toEqual(["ccarneromerida", "next-dvalverde", "laubernal"]);
  expect(raw[0]).toBe(" CCarneroMerida ");
  const a = {...fresh, account:"alice", has_token:true, authors:["alice"], pull_request_authors:["first"]};
  const b = {...a, id:"b", pull_request_authors:["second"]};
  const cleaned = cleanSourceConnectors([a,b]);
  cleaned[0].pull_request_authors!.push("third");
  expect(a.pull_request_authors).toEqual(["first"]);
  expect(cleaned[1].pull_request_authors).toEqual(["second"]);
  expect(cleaned[0].authors).toEqual(["alice"]);
  const {pull_request_authors: removed, ...legacy} = a;
  expect(removed).toEqual(["first"]);
  expect(validateSourceConnectors([legacy])).toEqual([]);
  expect(cleanSourceConnectors([legacy])[0].pull_request_authors).toEqual([]);
  expect("pull_request_authors" in legacy).toBe(false);
});

it("rejects malformed PR author fields and invalid GitHub logins", () => {
  for (const value of [null, "alice", 1, {}, [1], ["team/repo"], ["with space"]]) {
    const invalid = {...newGitHubIssuesSource("a"), account:"alice", has_token:true,
      pull_request_authors:value} as unknown as SourceConnector;
    expect(validateSourceConnectors([invalid]).join(" ")).toContain("pull_request_authors");
  }
});
