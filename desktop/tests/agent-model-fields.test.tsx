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
  expect((await screen.findByRole("alert")).textContent).toContain("CLI unavailable");
  expect((screen.getByLabelText("Model") as HTMLSelectElement).value).toBe("saved");
  expect(change).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button",{name:"Refresh agent models"}));
  await screen.findByRole("option",{name:"Saved"});
  expect((screen.getByLabelText("Effort") as HTMLSelectElement).disabled).toBe(false);
  expect((screen.getByLabelText("Effort") as HTMLSelectElement).value).toBe("high");
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
