import { beforeAll, afterEach, it, expect, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SettingsPage } from "../src/context/factory/infrastructure/presentations/settings";
import { FlowsPage } from "../src/context/factory/infrastructure/presentations/flows";
import type { Settings } from "../src/context/factory/domain/types";
beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);
const settings = (): Settings => ({
  owners: [],
  authors: [],
  reviewers: [],
  token_profiles: [
    { id: "work", label: "Work", account: "worker", has_token: true },
  ],
  projects: [],
  commands: { codex: "codex", opencode: "opencode", git: "git" },
  poll_interval_seconds: 15,
  max_concurrent: 4,
  node_timeout_seconds: 3600,
  start_at_login: false,
  flows: [
    {
      id: "new_issue",
      name: "Issue",
      enabled: true,
      nodes: [
        {
          id: "plan",
          name: "Plan",
          agent: "codex",
          model: "",
          effort: "",
          access: "write",
          prompt: "Plan task",
          result_schema: { type: "object" },
          depends_on: [],
          position: { x: 0, y: 0 },
        },
      ],
    },
    { id: "pr_approved", name: "Approved", enabled: false, nodes: [] },
    { id: "pr_review", name: "Review", enabled: false, nodes: [] },
  ],
});
it("explains temporary workspaces without local repository controls", () => {
  const value = settings();
  value.projects = [
    { id: "legacy", repository: "owner/repo", source_path: "/old/repo" },
  ];
  render(
    <SettingsPage settings={value} onSave={vi.fn()} appDir="/app/data" />,
  );

  expect(screen.getByRole("heading", { name: "Temporary workspaces" })).toBeTruthy();
  expect(screen.getByText(/cloned automatically from GitHub for each run/i)).toBeTruthy();
  expect(screen.getByText("/app/data/workspaces/<run-id>")).toBeTruthy();
  expect(screen.getByText(/deleted after success, failure, or cancellation/i)).toBeTruthy();
  expect(screen.getByText(/logs, results, and declared artifacts are kept outside/i)).toBeTruthy();
  expect(screen.queryByLabelText("Local folder")).toBeNull();
  expect(screen.queryByRole("button", { name: "Add project" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Choose local folder" })).toBeNull();
});

it("preserves legacy projects when saving unrelated settings", async () => {
  const value = settings();
  value.projects = [
    { id: "legacy", repository: "owner/repo", source_path: "unused/path" },
  ];
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <SettingsPage settings={value} onSave={save} appDir="/app/data" />,
  );

  fireEvent.change(screen.getByLabelText("GitHub owners"), {
    target: { value: "owner" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));

  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].projects).toEqual(value.projects);
});

it("keeps account tokens when replacement is typed then cleared", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <SettingsPage settings={settings()} onSave={save} appDir="/app/data" />,
  );
  const user = userEvent.setup();
  const secret = screen.getByLabelText("Replace token");
  await user.type(secret, "replacement");
  await user.clear(secret);
  await user.clear(screen.getByLabelText("Profile name"));
  await user.type(screen.getByLabelText("Profile name"), "New label");
  await user.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].token_profiles[0].token).toBeUndefined();
});
it("allows comma separated owners while typing", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <SettingsPage settings={settings()} onSave={save} appDir="/app/data" />,
  );
  const user = userEvent.setup();
  await user.type(screen.getByLabelText("GitHub owners"), "one, two");
  await user.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].owners).toEqual(["one", "two"]);
});
it("blocks saving invalid schema text and keeps invalid draft on node selection", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<FlowsPage settings={settings()} onSave={save} />);
  const user = userEvent.setup();
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "Updated" },
  });
  const schema = screen.getByLabelText("Result schema · JSON");
  fireEvent.change(schema, { target: { value: "{" } });
  await user.click(screen.getByRole("button", { name: "Save workflow" }));
  expect(save).not.toHaveBeenCalled();
  expect(screen.getByRole("alert").textContent).toMatch(/valid JSON/i);
});

it('requires a replacement token when changing an existing profile account', async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={settings()} onSave={save} appDir="/app/data" />);
  fireEvent.change(screen.getByLabelText('GitHub account'), {target: {value: 'another-account'}});
  fireEvent.click(screen.getByRole('button', {name: 'Save changes'}));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/token/i));
  expect(save).not.toHaveBeenCalled();
});

vi.mock("../src/context/factory/infrastructure/ipc/factory-api", async (original) => {
  const actual = await original<typeof import("../src/context/factory/infrastructure/ipc/factory-api")>();
  return { ...actual, factoryApi: { ...actual.factoryApi, agentModels: vi.fn(async (agent: string) => agent === "codex" ? [
    {id: "codex-a", name: "Codex A", efforts: ["low", "high"]},
    {id: "codex-b", name: "Codex B", efforts: ["medium"]},
  ] : [{id: "provider/open-model", name: "Open model", efforts: ["fast", "deep"]}]) } };
});

it("selects models and efforts from the agent catalog and clears incompatible choices", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<FlowsPage settings={settings()} onSave={save} />);
  const user = userEvent.setup();
  fireEvent.click(screen.getByText("Plan", {exact: true}));
  const model = await screen.findByRole("combobox", {name: "Model"});
  await screen.findByRole("option", {name: "Codex A"});
  await user.selectOptions(model, "codex-a");
  const effort = screen.getByRole("combobox", {name: "Effort"});
  expect(Array.from((effort as HTMLSelectElement).options).map(o => o.value)).toEqual(["", "low", "high"]);
  await user.selectOptions(effort, "high");
  await user.selectOptions(model, "codex-b");
  expect((effort as HTMLSelectElement).value).toBe("");
  expect(Array.from((effort as HTMLSelectElement).options).map(o => o.value)).toEqual(["", "medium"]);
  await user.selectOptions(screen.getByLabelText("Agent"), "opencode");
  await screen.findByRole("option", {name: "Open model (provider/open-model)"});
  expect((model as HTMLSelectElement).value).toBe("");
  expect(screen.queryByRole("option", {name: "Codex A"})).toBeNull();
  await user.selectOptions(model, "provider/open-model");
  expect(Array.from((effort as HTMLSelectElement).options).map(o => o.value)).toEqual(["", "fast", "deep"]);
  await user.selectOptions(effort, "deep");
  await user.click(screen.getByRole("button", {name: "Save workflow"}));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].flows[0].nodes[0]).toMatchObject({agent: "opencode", model: "provider/open-model", effort: "deep"});
});
