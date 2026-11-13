import { afterEach, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AgentModelFields } from "../src/context/factory/infrastructure/presentations/agent-model-fields";
import { factoryApi } from "../src/context/factory/infrastructure/ipc/factory-api";
import type { AgentModel, FlowNode } from "../src/context/factory/domain/types";
vi.mock("../src/context/factory/infrastructure/ipc/factory-api", () => ({ factoryApi: { agentModels: vi.fn() } }));
afterEach(() => {cleanup(); vi.resetAllMocks();});
const node: FlowNode = {id:"step",name:"Step",agent:"codex",model:"",effort:"",access:"read",prompt:"Do work",depends_on:[],result_schema:{},position:{x:0,y:0}};

it("ignores a delayed model response from the previous agent", async () => {
  let finish!: (value: AgentModel[]) => void;
  vi.mocked(factoryApi.agentModels).mockImplementationOnce(() => new Promise(resolve => { finish=resolve; })).mockResolvedValueOnce([{id:"provider/open",name:"Open",efforts:["deep"]}]);
  const change=vi.fn();
  const view=render(<AgentModelFields node={node} command="codex" onChange={change} />);
  view.rerender(<AgentModelFields node={{...node,agent:"opencode"}} command="opencode" onChange={change} />);
  await screen.findByRole("option",{name:"Open (provider/open)"});
  await act(async()=>finish([{id:"late-codex",name:"Late Codex",efforts:["high"]}]));
  expect(screen.queryByRole("option",{name:"Late Codex"})).toBeNull();
  expect(screen.getByRole("option",{name:"Open (provider/open)"})).toBeDefined();
  expect(change).not.toHaveBeenCalled();
});
it("keeps saved choices on discovery failure and allows retry", async () => {
  vi.mocked(factoryApi.agentModels).mockRejectedValueOnce(new Error("CLI unavailable")).mockResolvedValueOnce([{id:"saved",name:"Saved",efforts:["high"]}]);
  const change=vi.fn();
  render(<AgentModelFields node={{...node,model:"saved",effort:"high"}} command="codex" onChange={change} />);
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Could not load Codex models.");
  expect(alert.textContent).toContain("CLI unavailable");
  expect((screen.getByLabelText("Model") as HTMLSelectElement).value).toBe("saved");
  expect(change).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button",{name:"Refresh agent models"}));
  await screen.findByRole("option",{name:"Saved"});
  expect((screen.getByLabelText("Effort") as HTMLSelectElement).disabled).toBe(false);
  expect((screen.getByLabelText("Effort") as HTMLSelectElement).value).toBe("high");
});
it("does not show a previous command error while the current catalog is loading", async () => {
  let finish!: (value: AgentModel[]) => void;
  vi.mocked(factoryApi.agentModels)
    .mockRejectedValueOnce(new Error("old command denied"))
    .mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const view = render(<AgentModelFields node={node} command="codex" onChange={vi.fn()} />);
  await screen.findByText(/old command denied/);
  view.rerender(<AgentModelFields node={node} command="codex --new-profile" onChange={vi.fn()} />);
  expect((await screen.findByRole("status")).textContent).toContain("Loading Codex models…");
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByText(/old command denied/)).toBeNull();
  await act(async () => finish([]));
});
it("offers only default effort when the selected model has no variants", async () => {
  vi.mocked(factoryApi.agentModels).mockResolvedValue([{id:"plain",name:"Plain",efforts:[]}]);
  render(<AgentModelFields node={{...node,model:"plain"}} command="codex" onChange={vi.fn()} />);
  await screen.findByRole("option",{name:"Plain"});
  const effort=screen.getByLabelText("Effort") as HTMLSelectElement;
  expect(effort.disabled).toBe(true);
  expect(Array.from(effort.options).map(o=>o.value)).toEqual([""]);
});
it("lets the user clear an obsolete saved effort even when the model has no variants", async () => {
  vi.mocked(factoryApi.agentModels).mockResolvedValue([{id:"plain",name:"Plain",efforts:[]}]);
  const change=vi.fn();
  render(<AgentModelFields node={{...node,model:"plain",effort:"obsolete"}} command="codex" onChange={change} />);
  await screen.findByRole("option",{name:"Plain"});
  const effort=screen.getByLabelText("Effort") as HTMLSelectElement;
  expect(effort.disabled).toBe(false);
  fireEvent.change(effort,{target:{value:""}});
  expect(change).toHaveBeenCalledWith({effort:""});
});

it("refreshes catalogs when GitHub profile changes and ignores the previous account response", async () => {
  let finish!: (value: AgentModel[]) => void;
  vi.mocked(factoryApi.agentModels)
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockResolvedValue([{ id: "personal-model", name: "Personal model", efforts: [] }]);
  const view = render(<AgentModelFields node={node} command="codex" tokenProfileId="work" onChange={vi.fn()} />);
  expect(factoryApi.agentModels).toHaveBeenLastCalledWith("codex", "work");
  view.rerender(<AgentModelFields node={node} command="codex" tokenProfileId="personal" onChange={vi.fn()} />);
  await screen.findByRole("option", { name: "Personal model" });
  expect(factoryApi.agentModels).toHaveBeenLastCalledWith("codex", "personal");
  await act(async () => finish([{ id: "work-model", name: "Work model", efforts: [] }]));
  expect(screen.queryByRole("option", { name: "Work model" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Refresh agent models" }));
  expect(factoryApi.agentModels).toHaveBeenLastCalledWith("codex", "personal");
  await act(async () => {});
});

it("loads each GitHub override with its own profile scope", async () => {
  const { TokenAgentFields } = await import("../src/context/factory/infrastructure/presentations/token-agent-fields");
  vi.mocked(factoryApi.agentModels).mockResolvedValue([]);
  const profile = { label: "Work", account: "worker", has_token: true, owners: [], authors: [], reviewers: [] };
  render(<TokenAgentFields node={{ ...node, token_overrides: { work: { agent: "codex", model: "", effort: "" }, personal: { agent: "codex", model: "", effort: "" } } }} profiles={[{ ...profile, id: "work" }, { ...profile, id: "personal" }]} commands={{ codex: "codex", opencode: "opencode", git: "git" }} onChange={vi.fn()} />);
  expect(factoryApi.agentModels).toHaveBeenCalledWith("codex", "work");
  expect(factoryApi.agentModels).toHaveBeenCalledWith("codex", "personal");
  await act(async () => {});
});

it("invalidates a profile catalog after rebinding to another Codex account", async () => {
  let finish!: (value: AgentModel[]) => void;
  vi.mocked(factoryApi.agentModels)
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockResolvedValue([{ id: "account-b-model", name: "Account B model", efforts: [] }]);
  const view = render(<AgentModelFields node={node} command="codex" tokenProfileId="work" boundAccountId="account-a" onChange={vi.fn()} />);
  view.rerender(<AgentModelFields node={node} command="codex" tokenProfileId="work" boundAccountId="account-b" onChange={vi.fn()} />);
  await screen.findByRole("option", { name: "Account B model" });
  expect(factoryApi.agentModels).toHaveBeenCalledTimes(2);
  expect(factoryApi.agentModels).toHaveBeenLastCalledWith("codex", "work");
  await act(async () => finish([{ id: "account-a-model", name: "Account A model", efforts: [] }]));
  expect(screen.queryByRole("option", { name: "Account A model" })).toBeNull();
});
