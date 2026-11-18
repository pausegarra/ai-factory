import { newGitHubIssuesSource } from "../src/context/factory/domain/source-connectors";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within, waitFor } from "@testing-library/react";
import { SourcesPage } from "../src/context/factory/infrastructure/presentations/sources";
import { noticeSettings, deferred } from "./fixtures/notices";
import type { Settings } from "../src/context/factory/domain/types";
afterEach(cleanup);
function add() {
  fireEvent.click(screen.getByRole("button", {name:"Add connector"}));
  const picker = within(screen.getByRole("dialog", {name:"Choose a connector"}));
  expect(picker.getAllByRole("button")).toHaveLength(2);
  expect(picker.getByRole("button", {name:"GitHub Issues"}).querySelector("svg")).toBeTruthy();
  fireEvent.click(picker.getByRole("button", {name:"GitHub Issues"}));
}
function fill(label="Work", token="fixture-token") {
  fireEvent.change(screen.getByLabelText("Name"), {target:{value:label}});
  fireEvent.change(screen.getByLabelText("GitHub account"), {target:{value:"alice"}});
  fireEvent.change(screen.getByLabelText("API token"), {target:{value:token}});
  fireEvent.change(screen.getByLabelText("GitHub owners"), {target:{value:" team, , second "}});
  fireEvent.change(screen.getByLabelText("Allowed authors"), {target:{value:"alice"}});
}
function apply() {fireEvent.click(screen.getByRole("button", {name:"Apply changes"}));}
function open(label="Work") {fireEvent.click(screen.getByRole("button", {name:`Edit connector ${label}`}));}
function save() {fireEvent.click(screen.getByRole("button", {name:"Save changes"}));}
it("adds independent same-account sources, applies before saving and clears secrets after void success", async () => {
 const onSave=vi.fn(async (_s:Settings)=>{});
 render(<SourcesPage settings={noticeSettings()} onSave={onSave}/>);
 expect(screen.getByText("No source connectors")).toBeTruthy();
 add(); fill(); apply(); expect(onSave).not.toHaveBeenCalled();
 add(); fill("Personal","fixture-second"); apply(); save();
 await screen.findByText("Source settings saved.");
 const sources=onSave.mock.calls[0][0].source_connectors;
 expect(sources).toHaveLength(2); expect(sources[0].id).not.toBe(sources[1].id);
 expect(sources[0].owners).toEqual(["team","second"]);
 expect(sources[1].token).toBe("fixture-second");
 open(); expect((screen.getByLabelText("API token") as HTMLInputElement).value).toBe("");
 expect(screen.getByText("Token configured")).toBeTruthy();
 fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}}); apply(); save();
 await waitFor(()=>expect(onSave).toHaveBeenCalledTimes(2));
 expect(onSave.mock.calls[1][0].source_connectors[0].token).toBeUndefined();
});
it("cancels, removes and discards drafts with focus restoration", async()=>{
 render(<SourcesPage settings={noticeSettings()} onSave={vi.fn()}/>);
 add(); fill(); fireEvent.keyDown(screen.getByRole("dialog"),{key:"Escape"});
 expect(screen.getByText("No source connectors")).toBeTruthy();
 add(); fill(); apply(); open();
 fireEvent.click(screen.getByRole("button",{name:"Remove connector Work"}));
 await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
 expect(screen.getByText("No source connectors")).toBeTruthy();
});
it("retains pending secrets on sanitized failure and refresh, prevents double save and preserves current unrelated settings",async()=>{
 const pending=deferred<Settings>();const onSave=vi.fn((_s:Settings)=>pending.promise);
 const original=noticeSettings(); const view=render(<SourcesPage settings={original} onSave={onSave}/>);
 add(); fill(); apply(); view.rerender(<SourcesPage settings={{...original,max_concurrent:9}} onSave={onSave}/>);
 const saveButton=screen.getByRole("button",{name:"Save changes"}); fireEvent.click(saveButton);fireEvent.click(saveButton);expect(onSave).toHaveBeenCalledTimes(1);
 expect(onSave.mock.calls[0][0].max_concurrent).toBe(9);
 expect(screen.getByRole("button",{name:"Add connector"}).matches(":disabled")).toBe(true);
 pending.reject(new Error("fixture-token private-url"));
 await screen.findByRole("alert");expect(screen.getByRole("alert").textContent).not.toContain("fixture-token");
 open();expect((screen.getByLabelText("API token") as HTMLInputElement).value).toBe("fixture-token");
 fireEvent.click(screen.getByRole("button",{name:"Cancel"}));
 fireEvent.click(screen.getByRole("button",{name:"Discard changes"}));
 expect(screen.getByText("No source connectors")).toBeTruthy();
});
it("validates incomplete sources and requires a replacement after account change",async()=>{
 const onSave=vi.fn(async(s:Settings)=>s);
 render(<SourcesPage settings={noticeSettings()} onSave={onSave}/>);
 add();apply();save();expect(onSave).not.toHaveBeenCalled();
 expect(within(screen.getByRole("dialog")).getByRole("alert")).toBeTruthy();
 fill();apply();save(); await screen.findByText("Source settings saved.");
 open();fireEvent.change(screen.getByLabelText("GitHub account"),{target:{value:"bob"}});apply();save();
 expect(onSave).toHaveBeenCalledTimes(1);
 expect(within(screen.getByRole("dialog")).getByRole("alert").textContent).toMatch(/token/i);
});

it("defaults to requested reviews and reloads both saved scope values", async () => {
  const onSave = vi.fn(async (settings:Settings) => structuredClone(settings));
  const view = render(<SourcesPage settings={noticeSettings()} onSave={onSave}/>);
  add(); fill();
  const scope = () => screen.getByRole("checkbox", {
    name:"Review all open PRs in configured owners"
  }) as HTMLInputElement;
  expect(scope().checked).toBe(false);
  fireEvent.click(scope()); apply(); save();
  await screen.findByText("Source settings saved.");
  expect(onSave.mock.calls[0][0].source_connectors[0].review_all_open_pull_requests)
    .toBe(true);
  const stored = structuredClone(onSave.mock.calls[0][0]);
  stored.source_connectors[0].has_token = true;
  delete stored.source_connectors[0].token;
  view.unmount();

  render(<SourcesPage settings={stored} onSave={onSave}/>);
  open();
  expect(scope().checked).toBe(true);
  expect((screen.getByLabelText("API token") as HTMLInputElement).value).toBe("");
  fireEvent.click(scope()); apply(); save();
  await screen.findByText("Source settings saved.");
  expect(onSave.mock.calls[1][0].source_connectors[0].review_all_open_pull_requests)
    .toBe(false);
  expect(onSave.mock.calls[1][0].source_connectors[0].token).toBeUndefined();
  open();
  expect(scope().checked).toBe(false);
});

it("keeps old settings valid and cancels scope changes with the existing editor", async () => {
  const {review_all_open_pull_requests: _scope, ...legacy} =
    {...newGitHubIssuesSource("work"), label:"Work", account:"alice",
      has_token:true, owners:["team"], reviewers:["alice"]};
  const onSave = vi.fn(async (settings:Settings) => settings);
  render(<SourcesPage settings={{...noticeSettings(),source_connectors:[legacy]}}
    onSave={onSave}/>);
  const scope = () => screen.getByRole("checkbox", {
    name:"Review all open PRs in configured owners"
  }) as HTMLInputElement;
  open();
  expect(scope().checked).toBe(false);
  fireEvent.click(scope());
  fireEvent.click(screen.getByRole("button",{name:"Cancel"}));
  open();
  expect(scope().checked).toBe(false);
  fireEvent.change(screen.getByLabelText("Name"),{target:{value:"Renamed"}});
  apply(); save();
  await screen.findByText("Source settings saved.");
  expect(onSave.mock.calls[0][0].source_connectors[0].review_all_open_pull_requests)
    .toBe(false);
  expect(onSave.mock.calls[0][0].source_connectors[0].token).toBeUndefined();
});
