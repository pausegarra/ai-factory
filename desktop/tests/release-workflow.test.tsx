import {afterEach,it,expect,vi} from "vitest";
import {cleanup,fireEvent,render,screen,waitFor} from "@testing-library/react";
import type {FlowNode,Settings} from "../src/context/factory/domain/types";
const mocks=vi.hoisted(()=>({fitView:vi.fn()}));
vi.mock("../src/context/factory/infrastructure/ipc/factory-api",()=>({factoryApi:{agentModels:vi.fn(async()=>[{id:"fixture-model",name:"Fixture model",efforts:["low"]}])}}));
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
import { noticeSettings } from "./fixtures/notices";
import {FlowsPage} from "../src/context/factory/infrastructure/presentations/flows";
afterEach(cleanup);

function original(): Settings {
  return {...noticeSettings(), flows:[
    {id:"new_issue",name:"Tasks",enabled:true,nodes:[]},
    {id:"pr_approved",name:"Approvals",enabled:true,nodes:[]},
    {id:"pr_review",name:"Reviews",enabled:true,nodes:[]},
    {id:"release",name:"Release",enabled:false,nodes:[{id:"publish_app",name:"Publish app",agent:"codex",model:"",effort:"",access:"write",prompt:"Publish app with installed skill",result_schema:{type:"object"},depends_on:[],position:{x:80,y:180}}]},
  ]};
}
it("activates, edits, connects and reloads Release independently of the other flows", async()=>{
  const initial=original();const save=vi.fn(async(_s:Settings)=>{});
  const view=render(<FlowsPage settings={initial} onSave={save}/>);
  fireEvent.click(screen.getByRole("button",{name:/Release/}));
  const global=screen.getByRole("checkbox",{name:"Enable workflow globally for release"});
  expect((global as HTMLInputElement).checked).toBe(false);fireEvent.click(global);
  fireEvent.click(screen.getByRole("button",{name:"Add step"}));
  fireEvent.change(screen.getByLabelText("Prompt"),{target:{value:"Verify distribution receipt"}});
  fireEvent.click(screen.getByRole("button",{name:"Save workflow"}));
  await waitFor(()=>expect(save).toHaveBeenCalledOnce());
  const first=save.mock.calls[0][0];const release=first.flows.find(f=>f.id==="release")!;
  expect(release.enabled).toBe(true);expect(release.nodes).toHaveLength(2);
  const added=release.nodes.find(n=>n.id!=="publish_app")!.id;
  expect(release.nodes.find(n=>n.id===added)!.prompt).toBe("Verify distribution receipt");
  expect(first.flows.filter(f=>f.id!=="release")).toEqual(initial.flows.filter(f=>f.id!=="release"));
  view.rerender(<FlowsPage settings={first} onSave={save}/>);
  fireEvent.click(screen.getByTestId("node-publish_app"));
  expect(screen.queryByRole("button", { name: /select skills/i })).toBeNull();
  expect(screen.queryByRole("dialog", { name: /select skills/i })).toBeNull();
  fireEvent.change(screen.getByLabelText("Prompt"),{target:{value:"Use configured distribution skill"}});
  await screen.findByRole("option",{name:"Fixture model"});
  fireEvent.change(screen.getByLabelText("Model"),{target:{value:"fixture-model"}});
  fireEvent.change(screen.getByLabelText("Workspace access"),{target:{value:"read"}});
  fireEvent.click(screen.getByRole("button",{name:`Connect publish_app to ${added}`}));
  fireEvent.click(screen.getByRole("button",{name:"Save workflow"}));
  await waitFor(()=>expect(save).toHaveBeenCalledTimes(2));
  const second=save.mock.calls[1][0];expect(second.flows.find(f=>f.id==="release")!.nodes.find(n=>n.id===added)!.depends_on).toEqual(["publish_app"]);
  view.unmount();
  render(<FlowsPage settings={second} onSave={save}/>);
  fireEvent.click(screen.getByRole("button",{name:/Release/}));
  fireEvent.click(screen.getByTestId("node-publish_app"));
  expect(screen.queryByRole("button", { name: /select skills/i })).toBeNull();
  expect(screen.queryByRole("dialog", { name: /select skills/i })).toBeNull();
  expect((screen.getByLabelText("Prompt") as HTMLTextAreaElement).value).toBe("Use configured distribution skill");
  expect((screen.getByLabelText("Workspace access") as HTMLSelectElement).value).toBe("read");
  fireEvent.click(screen.getByRole("button",{name:`Delete publish_app::${added}`}));
  fireEvent.click(screen.getByTestId(`node-${added}`));fireEvent.click(screen.getByRole("button",{name:"Delete step"}));
  fireEvent.click(screen.getByRole("button",{name:"Save workflow"}));
  await waitFor(()=>expect(save).toHaveBeenCalledTimes(3));
  const final=save.mock.calls[2][0];
  for (const node of final.flows.flatMap((flow) => flow.nodes)) {
    expect(node).not.toHaveProperty("skills");
  }
  expect(final.flows.find(f=>f.id==="release")!.nodes).toEqual([{...initial.flows[3].nodes[0],prompt:"Use configured distribution skill",model:"fixture-model",access:"read",position:{x:80,y:80}}]);
  expect(final.flows.filter(f=>f.id!=="release")).toEqual(initial.flows.filter(f=>f.id!=="release"));
});
