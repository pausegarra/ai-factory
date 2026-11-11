import {it, expect} from "vitest";
import type {Flow, FlowNode} from "../src/context/factory/domain/types";
import {arrangeFlow, flowConnections, hasAutomaticLayout, displayFlow} from "../src/context/factory/application/flow-layout";
const step=(id:string,index:number,depends_on:string[]=[]):FlowNode=>({id,name:id,agent:"codex",model:"",effort:"",access:"read",prompt:"Review",result_schema:{},depends_on,position:{x:80+index*280,y:180}});
const fork=():Flow=>({id:"new_issue",name:"Issue",enabled:true,nodes:[step("builder",0),step("code",1,["builder"]),step("tests",2,["builder"]),step("architecture",3,["builder"]),step("publish",4,["code","tests","architecture"])]});
it("projects one branch per dependency, including fork and join",()=>{
  const edges=flowConnections(fork());
  expect(edges.filter(e=>e.source==="builder").map(e=>e.target)).toEqual(["code","tests","architecture"]);
  expect(edges.filter(e=>e.target==="publish")).toHaveLength(3);
});
it("places parallel children in distinct rows of one column and joins downstream",()=>{
  const flow=arrangeFlow(fork());
  const children=flow.nodes.slice(1,4);
  expect(new Set(children.map(n=>n.position.x)).size).toBe(1);
  expect(new Set(children.map(n=>n.position.y)).size).toBe(3);
  expect(children[0].position.x).toBeGreaterThan(flow.nodes[0].position.x);
  expect(flow.nodes[4].position.x).toBeGreaterThan(children[0].position.x);
  expect(arrangeFlow(flow)).toEqual(flow);
  expect(flowConnections(flow)).toEqual(flowConnections(fork()));
});
it("repairs generated legacy rows but preserves manually positioned workflows",()=>{
  const flow=fork(); expect(hasAutomaticLayout(flow)).toBe(true);
  const displayed=displayFlow(flow); expect(displayed.nodes[1].position.y).not.toBe(displayed.nodes[2].position.y);
  expect(hasAutomaticLayout(displayed)).toBe(true);
  displayed.nodes[1].position={x:123,y:456};
  expect(hasAutomaticLayout(displayed)).toBe(false);
  expect(displayFlow(displayed)).toBe(displayed);
});
it("rejects cycles instead of leaving the layout loop running",()=>{
  const flow=fork();flow.nodes[0].depends_on=["publish"];
  expect(()=>arrangeFlow(flow)).toThrow(/cycle/i);
});
