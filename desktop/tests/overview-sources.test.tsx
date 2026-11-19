import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { OverviewPage } from "../src/context/factory/infrastructure/presentations/overview";
import { attentionSettings, attentionSnapshot } from "./fixtures/attention";
afterEach(cleanup);
it("routes empty sources setup to Sources after a clean install or upgrade", () => {
  const onPage = vi.fn();
  render(<OverviewPage snapshot={attentionSnapshot([])}
    settings={{...attentionSettings, source_connectors: []}}
    onPage={onPage} onRun={() => {}} />);
  expect(screen.getByText("GitHub source connector")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", {name: "Set up Factory"}));
  expect(onPage).toHaveBeenCalledExactlyOnceWith("sources");
});
it("reacts to connector readiness without borrowing owners from another token", () => {
 const onPage=vi.fn();const base={...attentionSettings,source_connectors:[]};
 const view=render(<OverviewPage snapshot={attentionSnapshot([])} settings={base} onPage={onPage} onRun={()=>{}}/>);
 const token={id:"a",label:"Work",type:"github_issues" as const,account:"alice",has_token:true,owners:[],authors:[],reviewers:[]};
 view.rerender(<OverviewPage snapshot={attentionSnapshot([])} settings={{...base,source_connectors:[token,{...token,id:"b",has_token:false,owners:["team"]}]}} onPage={onPage} onRun={()=>{}}/>);
 expect(screen.getByRole("button",{name:"Set up Factory"})).toBeTruthy();
 view.rerender(<OverviewPage snapshot={attentionSnapshot([])} settings={{...base,source_connectors:[{...token,owners:["team"]}]}} onPage={onPage} onRun={()=>{}}/>);
 expect(screen.queryByRole("button",{name:"Set up Factory"})).toBeNull();
 view.rerender(<OverviewPage snapshot={attentionSnapshot([])} settings={base} onPage={onPage} onRun={()=>{}}/>);
 expect(screen.getByRole("button",{name:"Set up Factory"})).toBeTruthy();
});

it("shows the effective issue prefix and reacts to settings changes", () => {
  const props = { snapshot: attentionSnapshot([]), settings: attentionSettings, onPage: vi.fn(), onRun: vi.fn() };
  const view = render(<OverviewPage {...props} />);
  expect(screen.getByText("Open an issue whose title starts with [AI] in a connected repository. Your workflow starts automatically.")).toBeTruthy();
  view.rerender(<OverviewPage {...props} settings={{ ...attentionSettings, issue_title_prefix: "bot:" }} />);
  expect(screen.getByText("Open an issue whose title starts with bot: in a connected repository. Your workflow starts automatically.")).toBeTruthy();
  expect(screen.queryByText(/Open an issue.*\[AI\]/)).toBeNull();
});
