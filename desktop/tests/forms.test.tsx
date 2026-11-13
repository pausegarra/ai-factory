import { beforeAll, afterEach, it, expect, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SettingsPage } from "../src/context/factory/infrastructure/presentations/settings";
import { FlowsPage } from "../src/context/factory/infrastructure/presentations/flows";
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import type { Settings } from "../src/context/factory/domain/types";
beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);
function githubAccounts() {
  fireEvent.click(screen.getByRole("tab", { name: "GitHub accounts" }));
}
const settings = (): Settings => ({
  token_profiles: [
    { id: "work", label: "Work", account: "worker", has_token: true, owners: [], authors: [], reviewers: [] },
  ],
  projects: [],
  commands: { codex: "codex", opencode: "opencode", git: "git" },
  poll_interval_seconds: 15,
  max_concurrent: 4,
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
it("does not offer an agent timeout setting", () => {
  render(
    <SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app/data" />,
  );

  expect(screen.queryByLabelText(/step timeout/i)).toBeNull();
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

  githubAccounts();
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
  githubAccounts();
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
  githubAccounts();
  const user = userEvent.setup();
  await user.type(screen.getByLabelText("GitHub owners"), "one, two");
  await user.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].token_profiles[0].owners).toEqual(["one", "two"]);
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

const activationCheck = () => screen.getByRole("checkbox", { name: "Enable new_issue for Work (@worker)" }) as HTMLInputElement;
const globalCheck = () => screen.getByRole("checkbox", { name: "Enable workflow globally for new_issue" }) as HTMLInputElement;

it("keeps activation edits local, discards them and prevents dirty trigger switching", async () => {
  const value = settings();
  const dirty = vi.fn();
  const save = vi.fn();
  render(<FlowsPage settings={value} onSave={save} onDirtyChange={dirty} />);
  fireEvent.click(activationCheck());
  fireEvent.click(globalCheck());
  expect(save).not.toHaveBeenCalled();
  expect(value.flows[0].profile_activation).toBeUndefined();
  expect(value.flows[0].enabled).toBe(true);
  expect(dirty).toHaveBeenLastCalledWith(true);
  fireEvent.click(screen.getByRole("button", { name: /PR approved/ }));
  expect(screen.getByRole("alert").textContent).toContain("Save or discard");
  expect(activationCheck().checked).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect(activationCheck().checked).toBe(true);
  expect(globalCheck().checked).toBe(true);
  expect(dirty).toHaveBeenLastCalledWith(false);
  fireEvent.click(screen.getByRole("button", { name: /PR approved/ }));
  expect(screen.getByRole("checkbox", { name: "Enable workflow globally for pr_approved" })).toBeTruthy();
});

it("saves activation for selected trigger and preserves latest other flows and node overrides", async () => {
  const value = settings();
  value.flows[0].nodes[0].token_overrides = { work: { agent: "codex", model: "model", effort: "" } };
  const save = vi.fn().mockResolvedValue(undefined);
  const view = render(<FlowsPage settings={value} onSave={save} />);
  fireEvent.click(activationCheck());
  const latest = structuredClone(value);
  latest.flows[1].profile_activation = { work: false };
  view.rerender(<FlowsPage settings={latest} onSave={save} />);
  expect(activationCheck().checked).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0][0].flows[0]).toEqual({ ...value.flows[0], profile_activation: { work: false } });
  expect(save.mock.calls[0][0].flows.slice(1)).toEqual(latest.flows.slice(1));
  await screen.findByText("Workflow saved. New tasks will use this version.");
});

it("retains activation draft after save failure and allows correction and retry", async () => {
  const save = vi.fn().mockRejectedValueOnce(new Error("fixture save denied")).mockResolvedValue(undefined);
  const dirty = vi.fn();
  render(<FlowsPage settings={settings()} onSave={save} onDirtyChange={dirty} />);
  fireEvent.click(activationCheck());
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("fixture save denied"));
  expect(activationCheck().checked).toBe(false);
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect(screen.queryByText("Workflow saved. New tasks will use this version.")).toBeNull();
  fireEvent.click(globalCheck());
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1][0].flows[0]).toMatchObject({ enabled: false, profile_activation: { work: false } });
});

it("locks all native editor controls and draft mutations while save is pending", async () => {
  let resolve!: () => void;
  const save = vi.fn(() => new Promise<void>((done) => { resolve = done; }));
  const busy = vi.fn();
  const dirty = vi.fn();
  render(<FlowsPage settings={settings()} onSave={save} onBusyChange={busy} onDirtyChange={dirty} />);
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  fireEvent.click(activationCheck());
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(busy).toHaveBeenLastCalledWith(true));
  const editor = screen.getByRole("group", { name: "Workflow editor" }) as HTMLFieldSetElement;
  expect(editor.disabled).toBe(true);
  expect(editor.getAttribute("aria-busy")).toBe("true");
  expect((screen.getByRole("button", { name: "Saving…" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Discard" }) as HTMLButtonElement).disabled).toBe(true);
  const user = userEvent.setup();
  await user.click(globalCheck());
  await user.click(activationCheck());
  await user.click(screen.getByRole("button", { name: /PR approved/ }));
  await user.click(screen.getByRole("button", { name: "Add step" }));
  await user.click(screen.getByRole("button", { name: "Arrange flow" }));
  await user.click(screen.getByRole("button", { name: "Delete step" }));
  fireEvent.change(screen.getByLabelText("Result schema · JSON"), { target: { value: "{" } });
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "blocked" } });
  fireEvent.click(screen.getByRole("button", { name: "Saving…" }));
  expect(globalCheck().checked).toBe(true);
  expect(activationCheck().checked).toBe(false);
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Plan");
  expect((screen.getByLabelText("Result schema · JSON") as HTMLTextAreaElement).value).toContain('"type": "object"');
  expect(save).toHaveBeenCalledTimes(1);
  resolve();
  await screen.findByText("Workflow saved. New tasks will use this version.");
  expect(busy).toHaveBeenLastCalledWith(false);
  expect(dirty).toHaveBeenLastCalledWith(false);
});

it("preserves schema and activation drafts on settings events, but reflects clean updates", () => {
  const value = settings();
  const save = vi.fn();
  const view = render(<FlowsPage settings={value} onSave={save} />);
  const latest = structuredClone(value);
  latest.flows[0].profile_activation = { work: false };
  view.rerender(<FlowsPage settings={latest} onSave={save} />);
  expect(activationCheck().checked).toBe(false);
  fireEvent.click(activationCheck());
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  fireEvent.change(screen.getByLabelText("Result schema · JSON"), { target: { value: "{" } });
  view.rerender(<FlowsPage settings={value} onSave={save} />);
  expect(activationCheck().checked).toBe(true);
  expect((screen.getByLabelText("Result schema · JSON") as HTMLTextAreaElement).value).toBe("{");
});

it.each(["token", "poll interval"])("saves unrelated %s edits with a legacy dotted node ID and preserves activation", async (field) => {
  const value = settings();
  value.flows[0].nodes[0].id = "review.code";
  value.flows[0].profile_activation = { work: false, removed: true };
  value.flows[1].profile_activation = { work: true, removed: false };
  const original = structuredClone(value);
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={value} onSave={save} appDir="/app" />);

  if (field === "token") githubAccounts();
  fireEvent.change(screen.getByLabelText(field === "token" ? "Replace token" : "GitHub poll interval · seconds"), {
    target: { value: field === "token" ? "replacement-fixture" : "30" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));

  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const saved = save.mock.calls[0][0] as Settings;
  expect(saved.flows).toEqual(original.flows);
  if (field === "token") expect(saved.token_profiles[0].token).toBe("replacement-fixture");
  else expect(saved.poll_interval_seconds).toBe(30);
  expect(value).toEqual(original);
  expect(screen.queryByRole("alert")).toBeNull();
});

it("Settings validates latest activation metadata instead of stale draft flows", async () => {
  const value = settings();
  const save = vi.fn().mockResolvedValue(undefined);
  const view = render(<SettingsPage settings={value} onSave={save} appDir="/app" />);
  githubAccounts();
  fireEvent.change(screen.getByLabelText("GitHub owners"), { target: { value: "owner" } });
  const latest = structuredClone(value);
  latest.flows[0].profile_activation = { work: false };
  view.rerender(<SettingsPage settings={latest} onSave={save} appDir="/app" />);
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0][0]).toMatchObject({ token_profiles: [{ ...value.token_profiles[0], owners: ["owner"] }], flows: latest.flows });
  latest.flows[0].profile_activation = { work: "bad" } as unknown as Record<string, boolean>;
  view.rerender(<SettingsPage settings={structuredClone(latest)} onSave={save} appDir="/app" />);
  fireEvent.change(screen.getByLabelText("GitHub owners"), { target: { value: "other" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(screen.getByRole("alert").textContent).toMatch(/activation must contain booleans/);
  expect(save).toHaveBeenCalledTimes(1);
});

it('requires a replacement token when changing an existing profile account', async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={settings()} onSave={save} appDir="/app/data" />);
  githubAccounts();
  fireEvent.change(screen.getByLabelText('GitHub account'), {target: {value: 'another-account'}});
  fireEvent.click(screen.getByRole('button', {name: 'Save changes'}));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/token/i));
  expect(save).not.toHaveBeenCalled();
});

vi.mock("../src/context/factory/infrastructure/ipc/factory-api", async (original) => {
  const actual = await original<typeof import("../src/context/factory/infrastructure/ipc/factory-api")>();
  return { ...actual, native: true, factoryApi: { ...actual.factoryApi, settings: vi.fn(), importLegacy: vi.fn(), agentModels: vi.fn(async (agent: string) => agent === "codex" ? [
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

it("saves independent agent configurations for every node and GitHub token", async () => {
  const { within } = await import("@testing-library/react");
  const value = settings();
  value.token_profiles.push({ id: "personal", label: "Personal", account: "bob", has_token: true, owners: [], authors: [], reviewers: [] });
  value.flows[0].nodes.push({ ...value.flows[0].nodes[0], id: "build", name: "Build", position: { x: 300, y: 0 } });
  const save = vi.fn().mockResolvedValue(undefined);
  const view = render(<FlowsPage settings={value} onSave={save} />);
  const user = userEvent.setup();
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  const work = within(screen.getByRole("group", { name: "Work (@worker)" }));
  const personal = within(screen.getByRole("group", { name: "Personal (@bob)" }));
  expect((work.getByLabelText("Agent for Work") as HTMLSelectElement).value).toBe("");
  expect(work.queryByLabelText("Model")).toBeNull();
  await user.selectOptions(work.getByLabelText("Agent for Work"), "opencode");
  await work.findByRole("option", { name: "Open model (provider/open-model)" });
  await user.selectOptions(work.getByLabelText("Model"), "provider/open-model");
  await user.selectOptions(work.getByLabelText("Effort"), "deep");
  await user.selectOptions(personal.getByLabelText("Agent for Personal"), "codex");
  await personal.findByRole("option", { name: "Codex A" });
  await user.selectOptions(personal.getByLabelText("Model"), "codex-a");
  await user.selectOptions(personal.getByLabelText("Effort"), "low");

  fireEvent.click(screen.getByText("Build", { exact: true }));
  const buildWork = within(screen.getByRole("group", { name: "Work (@worker)" }));
  expect((buildWork.getByLabelText("Agent for Work") as HTMLSelectElement).value).toBe("");
  await user.selectOptions(buildWork.getByLabelText("Agent for Work"), "codex");
  await buildWork.findByRole("option", { name: "Codex B" });
  await user.selectOptions(buildWork.getByLabelText("Model"), "codex-b");
  await user.selectOptions(buildWork.getByLabelText("Effort"), "medium");
  await user.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const saved = save.mock.calls[0][0];
  expect(saved.flows[0].nodes[0].token_overrides).toEqual({
    work: { agent: "opencode", model: "provider/open-model", effort: "deep" },
    personal: { agent: "codex", model: "codex-a", effort: "low" },
  });
  expect(saved.flows[0].nodes[1].token_overrides).toEqual({
    work: { agent: "codex", model: "codex-b", effort: "medium" },
  });
  expect(saved.flows[0].nodes.map((node: { agent: string; model: string; effort: string }) => [node.agent, node.model, node.effort])).toEqual([["codex", "", ""], ["codex", "", ""]]);

  view.rerender(<FlowsPage settings={saved} onSave={save} />);
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  const restored = within(screen.getByRole("group", { name: "Work (@worker)" }));
  expect((restored.getByLabelText("Model") as HTMLSelectElement).value).toBe("provider/open-model");
  await user.selectOptions(restored.getByLabelText("Agent for Work"), "");
  expect(restored.queryByLabelText("Model")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1][0].flows[0].nodes[0].token_overrides).toEqual({ personal: saved.flows[0].nodes[0].token_overrides.personal });
  expect(save.mock.calls[1][0].flows[0].nodes[1].token_overrides).toEqual(saved.flows[0].nodes[1].token_overrides);
});

it("clears incompatible token choices without changing step defaults", async () => {
  const { within } = await import("@testing-library/react");
  const value = settings();
  value.flows[0].nodes[0].model = "codex-b";
  value.flows[0].nodes[0].effort = "medium";
  value.flows[0].nodes[0].token_overrides = { work: { agent: "codex", model: "codex-a", effort: "high" } };
  const save = vi.fn().mockResolvedValue(undefined);
  render(<FlowsPage settings={value} onSave={save} />);
  const user = userEvent.setup();
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  const work = within(screen.getByRole("group", { name: "Work (@worker)" }));
  await work.findByRole("option", { name: "Codex B" });
  await user.selectOptions(work.getByLabelText("Model"), "codex-b");
  expect((work.getByLabelText("Effort") as HTMLSelectElement).value).toBe("");
  await user.selectOptions(work.getByLabelText("Effort"), "medium");
  await user.selectOptions(work.getByLabelText("Agent for Work"), "opencode");
  await work.findByRole("option", { name: "Open model (provider/open-model)" });
  expect((work.getByLabelText("Model") as HTMLSelectElement).value).toBe("");
  expect((work.getByLabelText("Effort") as HTMLSelectElement).value).toBe("");
  await user.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].flows[0].nodes[0]).toMatchObject({
    agent: "codex", model: "codex-b", effort: "medium",
    token_overrides: { work: { agent: "opencode", model: "", effort: "" } },
  });
});

it("shows empty profile help and lets users remove inactive overrides", async () => {
  const value = settings();
  value.token_profiles = [];
  value.flows[0].nodes[0].token_overrides = { removed: { agent: "codex", model: "codex-a", effort: "high" } };
  const save = vi.fn().mockResolvedValue(undefined);
  render(<FlowsPage settings={value} onSave={save} />);
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  expect(screen.getByText(/Add GitHub token profiles in Settings/)).toBeTruthy();
  expect(screen.getByRole("group", { name: "Unavailable profile: removed" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Remove override for removed" }));
  expect(screen.queryByRole("group", { name: "Unavailable profile: removed" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0][0].flows[0].nodes[0].token_overrides).toEqual({});
});

it.each(["constructor", "__proto__"])("stores overrides for profile ID %s without inherited object properties", async (id) => {
  const { within } = await import("@testing-library/react");
  const value = settings();
  value.token_profiles[0].id = id;
  const save = vi.fn().mockResolvedValue(undefined);
  render(<FlowsPage settings={value} onSave={save} />);
  const user = userEvent.setup();
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  const work = within(screen.getByRole("group", { name: "Work (@worker)" }));
  expect(work.queryByLabelText("Model")).toBeNull();
  await user.selectOptions(work.getByLabelText("Agent for Work"), "opencode");
  await work.findByRole("option", { name: "Open model (provider/open-model)" });
  await user.click(screen.getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  const overrides = save.mock.calls[0][0].flows[0].nodes[0].token_overrides;
  expect(Object.keys(overrides)).toEqual([id]);
  expect(overrides[id]).toEqual({ agent: "opencode", model: "", effort: "" });
});


function twoAccountSettings(): Settings {
  const value = settings();
  value.token_profiles[0] = { ...value.token_profiles[0], owners: ["work-org"], authors: ["worker"], reviewers: [] };
  value.token_profiles.push({ id: "personal", label: "Personal", account: "alice", has_token: true, owners: ["alice"], authors: [], reviewers: ["alice"] });
  return value;
}
const accountFields = (name: string) => within(screen.getByRole("group", { name }));

it("saves independent account permission scopes while preserving comma drafts and tokens", async () => {
  const value = twoAccountSettings();
  const original = structuredClone(value);
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={value} onSave={save} appDir="/app" />);
  githubAccounts();
  const work = accountFields("Work (@worker)");
  const personal = accountFields("Personal (@alice)");
  const user = userEvent.setup();
  await user.type(work.getByLabelText("GitHub owners"), ", next-org, ");
  expect((work.getByLabelText("GitHub owners") as HTMLInputElement).value).toBe("work-org, next-org, ");
  fireEvent.change(personal.getByLabelText("Allowed authors"), { target: { value: "alice, worker" } });
  fireEvent.change(personal.getByLabelText("Allowed reviewers"), { target: { value: "" } });
  fireEvent.change(work.getByLabelText("Replace token"), { target: { value: "replacement-fixture" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const saved = save.mock.calls[0][0] as Settings;
  expect(saved.token_profiles[0]).toMatchObject({ owners: ["work-org", "next-org"], authors: ["worker"], reviewers: [], token: "replacement-fixture", has_token: true });
  expect(saved.token_profiles[1]).toMatchObject({ owners: ["alice"], authors: ["alice", "worker"], reviewers: [], has_token: true });
  expect(saved.token_profiles[1].token).toBeUndefined();
  for (const field of ["owners", "authors", "reviewers"]) expect(Object.hasOwn(saved, field)).toBe(false);
  expect(value).toEqual(original);
});

it("discards each account scope draft and leaves new accounts unscoped", () => {
  render(<SettingsPage settings={twoAccountSettings()} onSave={vi.fn()} appDir="/app" />);
  githubAccounts();
  fireEvent.change(accountFields("Work (@worker)").getByLabelText("GitHub owners"), { target: { value: "other-org, " } });
  fireEvent.change(accountFields("Personal (@alice)").getByLabelText("Allowed reviewers"), { target: { value: "worker, " } });
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect((accountFields("Work (@worker)").getByLabelText("GitHub owners") as HTMLInputElement).value).toBe("work-org");
  expect((accountFields("Personal (@alice)").getByLabelText("Allowed reviewers") as HTMLInputElement).value).toBe("alice");
  fireEvent.click(screen.getByRole("button", { name: "Add account" }));
  const fresh = accountFields("Account 3");
  for (const label of ["GitHub owners", "Allowed authors", "Allowed reviewers"]) expect((fresh.getByLabelText(label) as HTMLInputElement).value).toBe("");
});

it("reloads all per-account scope inputs after legacy import", async () => {
  const imported = twoAccountSettings();
  imported.token_profiles[0].owners = ["imported-org"];
  imported.token_profiles[1].authors = ["alice"];
  vi.mocked(factoryApi.settings).mockResolvedValue(imported);
  vi.mocked(factoryApi.importLegacy).mockResolvedValue({ runs_imported: 0, settings_imported: true, archive_path: "/archive", warnings: [] });
  render(<SettingsPage settings={settings()} onSave={vi.fn()} appDir="/app" />);
  fireEvent.change(screen.getByLabelText("Previous YAML config"), { target: { value: "/old/config.yaml" } });
  fireEvent.click(screen.getByRole("button", { name: "Import data" }));
  await screen.findByText(/Settings imported/);
  githubAccounts();
  await waitFor(() => expect((accountFields("Work (@worker)").getByLabelText("GitHub owners") as HTMLInputElement).value).toBe("imported-org"));
  expect((accountFields("Personal (@alice)").getByLabelText("Allowed authors") as HTMLInputElement).value).toBe("alice");
});

it.each(["GitHub owners", "Allowed authors", "Allowed reviewers"])("rejects malformed logins in per-account %s", async (label) => {
  const save = vi.fn();
  render(<SettingsPage settings={settings()} onSave={save} appDir="/app" />);
  githubAccounts();
  fireEvent.change(accountFields("Work (@worker)").getByLabelText(label), { target: { value: "valid-login, owner/repo" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(screen.getByRole("alert").textContent).toMatch(/GitHub login/);
  expect(save).not.toHaveBeenCalled();
});

it("preserves backend-compatible bot and legacy logins in each account scope", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<SettingsPage settings={settings()} onSave={save} appDir="/app" />);
  githubAccounts();
  for (const label of ["GitHub owners", "Allowed authors", "Allowed reviewers"]) {
    fireEvent.change(accountFields("Work (@worker)").getByLabelText(label), { target: { value: "dependabot[bot], legacy_login.name" } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0][0].token_profiles[0]).toMatchObject({
    owners: ["dependabot[bot]", "legacy_login.name"],
    authors: ["dependabot[bot]", "legacy_login.name"],
    reviewers: ["dependabot[bot]", "legacy_login.name"],
  });
});

it("reloads base and profile model catalogs when the bound Codex account changes", async () => {
  vi.mocked(factoryApi.agentModels).mockClear();
  const value = settings();
  value.codex_bindings = { work: "account-a" };
  value.flows[0].nodes[0].token_overrides = { work: { agent: "codex", model: "", effort: "" } };
  const view = render(<FlowsPage settings={value} onSave={vi.fn()} />);
  fireEvent.click(screen.getByText("Plan", { exact: true }));
  await waitFor(() => expect(factoryApi.agentModels).toHaveBeenCalledTimes(2));
  view.rerender(<FlowsPage settings={{ ...value, codex_bindings: { work: "account-b" } }} onSave={vi.fn()} />);
  await waitFor(() => expect(factoryApi.agentModels).toHaveBeenCalledTimes(4));
  expect(factoryApi.agentModels).toHaveBeenCalledWith("codex", "work");
});
