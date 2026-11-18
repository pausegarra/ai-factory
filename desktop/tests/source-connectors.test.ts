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
