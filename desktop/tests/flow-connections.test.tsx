import {afterEach,it,expect,vi} from "vitest";
import {cleanup,fireEvent,render,screen,waitFor} from "@testing-library/react";
import type {FlowNode,Settings} from "../src/context/factory/domain/types";
const mocks=vi.hoisted(()=>({fitView:vi.fn()}));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api",()=>({factoryApi:{agentModels:vi.fn(async()=>[])}}));
type CanvasNode={id:string;position:{x:number;y:number};data:{step:FlowNode}};
type CanvasEdge={id:string;source:string;target:string;selected?:boolean};
vi.mock("@xyflow/react",async(original)=>({
  ...(await original<typeof import("@xyflow/react")>()),
  Background:()=>null,Controls:()=>null,Handle:()=>null,useReactFlow:()=>mocks,
  ReactFlow:({nodes,edges,onConnect,onEdgesChange,onNodesChange,onNodeClick,onEdgeClick,children}:{nodes:CanvasNode[];edges:CanvasEdge[];onConnect:(c:{source:string;target:string;sourceHandle:null;targetHandle:null})=>void;onEdgesChange:(c:{id:string;type:"select"|"remove";selected?:boolean}[])=>void;onNodesChange:(c:{id:string;type:"remove"|"position";position?:{x:number;y:number}}[])=>void;onNodeClick:(e:null,n:CanvasNode)=>void;onEdgeClick?:(e:null,edge:CanvasEdge)=>void;children:React.ReactNode})=><div>
  <button onClick={()=>onNodesChange([{id:"builder",type:"position",position:{x:321,y:654}}])}>Move builder</button>
  <button onClick={()=>{onNodesChange([{id:"builder",type:"remove"}]);onEdgesChange(edges.filter(e=>e.source==="builder"||e.target==="builder").map(e=>({id:e.id,type:"remove"})));}}>Remove builder with connections</button>
  {nodes.map(n=><button key={n.id} onClick={()=>onNodeClick(null,n)} data-testid={`node-${n.id}`} data-x={n.position.x} data-y={n.position.y}>{n.data.step.name}</button>)}
  {nodes.flatMap(a=>nodes.filter(b=>a.id!==b.id).map(b=><button key={`${a.id}-${b.id}`} onClick={()=>onConnect({source:a.id,target:b.id,sourceHandle:null,targetHandle:null})}>Connect {a.id} to {b.id}</button>))}
  {edges.map(e=><div key={e.id} data-testid={e.id} data-selected={String(!!e.selected)}>
    <button onClick={()=>{onEdgesChange([{id:e.id,type:"select",selected:true}]);onEdgeClick?.(null,e);}}>Select {e.id}</button>
    <button onClick={()=>onEdgesChange([{id:e.id,type:"remove"}])}>Delete {e.id}</button>
  </div>)}{children}</div>,
}));
import {FlowsPage} from "../src/context/factory/infrastructure/presentations/flows";
afterEach(cleanup);
function settings(connected=false):Settings {
  const nodes:FlowNode[]=["builder","code","tests","architecture"].map((id,i)=>({id,name:id,agent:"codex",model:"",effort:"",access:"read",prompt:"Review",result_schema:{},depends_on:connected&&i>0?["builder"]:[],position:{x:80+i*280,y:180}}));
  return {owners:[],authors:[],reviewers:[],token_profiles:[],projects:[],commands:{codex:"codex",opencode:"opencode",git:"git"},poll_interval_seconds:15,node_timeout_seconds:3600,max_concurrent:4,start_at_login:false,flows:[{id:"new_issue",name:"Issue",enabled:true,nodes},{id:"pr_approved",name:"Approved",enabled:false,nodes:[]},{id:"pr_review",name:"Review",enabled:false,nodes:[]}]};
}
it("shows three distinct branches and no dependency selector in the inspector",async()=>{
  render(<FlowsPage settings={settings(true)} onSave={vi.fn()} />);
  expect(screen.getByTestId("builder::code")).toBeDefined();expect(screen.getByTestId("builder::tests")).toBeDefined();expect(screen.getByTestId("builder::architecture")).toBeDefined();
  const children=["code","tests","architecture"].map(id=>screen.getByTestId(`node-${id}`));
  expect(new Set(children.map(n=>n.getAttribute("data-y"))).size).toBe(3);
  fireEvent.click(screen.getByRole("button",{name:"code"}));
  await screen.findByText(/No models available/);
  expect(screen.queryByText("Depends on")).toBeNull();
  expect(screen.queryByRole("checkbox",{name:"builder"})).toBeNull();
});
it("connection edits are persisted, reloaded and removed through arrows",async()=>{
  const save=vi.fn(async(_settings:Settings)=>{});
  const view=render(<FlowsPage settings={settings()} onSave={save} />);
  for(const target of ["code","tests","architecture"]) fireEvent.click(screen.getByRole("button",{name:`Connect builder to ${target}`}));
  fireEvent.click(screen.getByRole("button",{name:"Save workflow"}));
  await waitFor(()=>expect(save).toHaveBeenCalled());
  const stored=save.mock.calls[0][0] as unknown as Settings;
  expect(stored.flows[0].nodes.slice(1).map(n=>n.depends_on)).toEqual([["builder"],["builder"],["builder"]]);
  view.unmount();save.mockClear();
  render(<FlowsPage settings={stored} onSave={save} />);
  fireEvent.click(screen.getByRole("button",{name:"Select builder::tests"}));
  expect(screen.getByTestId("builder::tests").getAttribute("data-selected")).toBe("true");
  fireEvent.click(screen.getByRole("button",{name:"Delete builder::tests"}));
  expect(screen.queryByTestId("builder::tests")).toBeNull();
  fireEvent.click(screen.getByRole("button",{name:"Save workflow"}));
  await waitFor(()=>expect(save).toHaveBeenCalled());
  const changed=save.mock.calls[0][0] as unknown as Settings;
  expect(changed.flows[0].nodes.find(n=>n.id==="tests")!.depends_on).toEqual([]);
  expect(changed.flows[0].nodes.find(n=>n.id==="code")!.depends_on).toEqual(["builder"]);
});
it("refuses a backwards connection that would create a cycle",()=>{
  render(<FlowsPage settings={settings(true)} onSave={vi.fn()} />);
  fireEvent.click(screen.getByRole("button",{name:"Connect code to builder"}));
  expect(screen.getByRole("alert").textContent).toMatch(/cycle/i);
  expect(screen.queryByTestId("code::builder")).toBeNull();
});

it("does not restore a deleted node when React Flow removes its arrows in the same event",()=>{
  render(<FlowsPage settings={settings(true)} onSave={vi.fn()} />);
  fireEvent.click(screen.getByRole("button",{name:"Remove builder with connections"}));
  expect(screen.queryByTestId("node-builder")).toBeNull();
  expect(screen.queryByTestId("builder::code")).toBeNull();
  expect(screen.getByTestId("node-code")).toBeDefined();
});
it("preserves saved manual coordinates until Arrange flow is requested",()=>{
  const manual=settings();manual.flows[0].nodes[0].position={x:123,y:456};
  render(<FlowsPage settings={manual} onSave={vi.fn()} />);
  fireEvent.click(screen.getByRole("button",{name:"Connect builder to code"}));
  expect(screen.getByTestId("node-builder").getAttribute("data-x")).toBe("123");
  expect(screen.getByTestId("node-builder").getAttribute("data-y")).toBe("456");
  fireEvent.click(screen.getByRole("button",{name:"Arrange flow"}));
  expect(screen.getByTestId("node-builder").getAttribute("data-x")).toBe("80");
});
it("saves dragged node positions and reloads their existing connections",async()=>{
  const save=vi.fn(async(_settings:Settings)=>{});
  const view=render(<FlowsPage settings={settings(true)} onSave={save} />);
  fireEvent.click(screen.getByRole("button",{name:"Move builder"}));
  expect(screen.getByTestId("node-builder").getAttribute("data-x")).toBe("321");
  expect(screen.getByTestId("node-builder").getAttribute("data-y")).toBe("654");
  fireEvent.click(screen.getByRole("button",{name:"Save workflow"}));
  await waitFor(()=>expect(save).toHaveBeenCalledOnce());
  const stored=save.mock.calls[0][0];
  expect(stored.flows[0].nodes[0].position).toEqual({x:321,y:654});
  expect(stored.flows[0].nodes.slice(1).map(n=>n.depends_on)).toEqual([["builder"],["builder"],["builder"]]);
  view.unmount();
  render(<FlowsPage settings={stored} onSave={save} />);
  expect(screen.getByTestId("node-builder").getAttribute("data-x")).toBe("321");
  expect(screen.getByTestId("node-builder").getAttribute("data-y")).toBe("654");
  for(const target of ["code","tests","architecture"]) expect(screen.getByTestId(`builder::${target}`)).toBeDefined();
});
