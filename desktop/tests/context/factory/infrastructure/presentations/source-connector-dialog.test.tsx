import { StrictMode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SourcesPage } from "../../../../../src/context/factory/infrastructure/presentations/sources";
import { SourceConnectorDialog } from "../../../../../src/context/factory/infrastructure/presentations/source-connector-dialog";
import { newGitHubIssuesSource } from "../../../../../src/context/factory/domain/source-connectors";
import { noticeSettings } from "../../../../fixtures/notices";
afterEach(cleanup);
const source = {...newGitHubIssuesSource("a"), label:"Work", account:"alice", has_token:true, owners:["team"]};
const initial = () => ({...noticeSettings(),source_connectors:[source]});
function open() {fireEvent.click(screen.getByRole("button",{name:"Edit connector Work"}));}
it("contains keyboard focus through StrictMode edits and restores the opener on Escape",async()=>{
 const user=userEvent.setup();render(<StrictMode><SourcesPage settings={initial()} onSave={vi.fn()}/></StrictMode>);
 const row=screen.getByRole("button",{name:"Edit connector Work"});row.focus();await user.keyboard("{Enter}");
 const dialog=screen.getByRole("dialog"),close=within(dialog).getByRole("button",{name:"Close connector editor"});
 expect(document.activeElement).toBe(close);expect(dialog.getAttribute("aria-modal")).toBe("true");expect(row.getAttribute("aria-controls")).toBe(dialog.id);
 await user.tab({shift:true});expect(document.activeElement).toBe(screen.getByRole("button",{name:"Apply changes"}));
 await user.tab();expect(document.activeElement).toBe(close);await user.tab();expect(document.activeElement).toBe(screen.getByLabelText("Name"));
 await user.type(screen.getByLabelText("Name")," draft");expect(document.activeElement).toBe(screen.getByLabelText("Name"));
 await user.keyboard("{Escape}");await waitFor(()=>expect(document.activeElement).toBe(row));
 open();expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Work");
});
it("keeps local secret edits during refresh and adopts deferred clean metadata on cancel",()=>{
 const original=initial(),onSave=vi.fn();const view=render(<SourcesPage settings={original} onSave={onSave}/>);open();
 fireEvent.change(screen.getByLabelText("API token"),{target:{value:"PRIVATE_LOCAL"}});
 fireEvent.change(screen.getByLabelText("New issue title prefix"),{target:{value:"Local:"}});
 const incoming={...original,source_connectors:[{...source,label:"Incoming",token:"PRIVATE_REMOTE",issue_title_prefix:"Incoming:"}]};
 view.rerender(<SourcesPage settings={incoming} onSave={onSave}/>);
 expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Work");
 expect(screen.getByLabelText<HTMLInputElement>("New issue title prefix").value).toBe("Local:");
 expect((screen.getByLabelText("API token") as HTMLInputElement).value).toBe("PRIVATE_LOCAL");expect(document.body.innerHTML).not.toContain("PRIVATE_REMOTE");
 fireEvent.click(screen.getByRole("button",{name:"Cancel"}));fireEvent.click(screen.getByRole("button",{name:"Edit connector Incoming"}));
 expect(screen.getByLabelText<HTMLInputElement>("New issue title prefix").value).toBe("Incoming:");
 expect((screen.getByLabelText("API token") as HTMLInputElement).value).toBe("");expect(onSave).not.toHaveBeenCalled();
});
it("adopts untouched external scopes and closes an editor after external removal",async()=>{
 const original=initial(),onSave=vi.fn();const view=render(<SourcesPage settings={original} onSave={onSave}/>);open();
 view.rerender(<SourcesPage settings={{...original,source_connectors:[{...source,owners:["external"]}]}} onSave={onSave}/>);
 expect((screen.getByLabelText("GitHub owners") as HTMLInputElement).value).toBe("external");
 view.rerender(<SourcesPage settings={noticeSettings()} onSave={onSave}/>);
 expect(screen.queryByRole("dialog")).toBeNull();await waitFor(()=>expect(document.activeElement).toBe(screen.getByRole("button",{name:"Add connector"})));
});
it("disables the entire editor and ignores Escape during a pending save",()=>{
 const cancel=vi.fn();render(<SourceConnectorDialog id="busy" connector={{...source,review_all_open_pull_requests:true,pull_request_authors:["first"]}} mode="existing" busy attempted={false} onApply={vi.fn()} onChange={vi.fn()} onCancel={cancel} onRemove={vi.fn()} returnFocus={()=>null}/>);
 const dialog=screen.getByRole("dialog");for(const control of dialog.querySelectorAll("button,input,select"))expect(control.matches(":disabled")).toBe(true);
 fireEvent.keyDown(dialog,{key:"Escape"});expect(cancel).not.toHaveBeenCalled();
});

it("labels the review scope, emits boolean patches and supports the keyboard", async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  const view = render(<SourceConnectorDialog id="scope" connector={source}
    mode="existing" busy={false} attempted={false} onApply={vi.fn()}
    onChange={changed} onCancel={vi.fn()} returnFocus={()=>null}/>);
  const control = screen.getByRole("checkbox", {
    name:"Review all open PRs in configured owners"
  }) as HTMLInputElement;
  expect(control.checked).toBe(false);
  expect(control.getAttribute("aria-describedby")).toBe("scope-review-scope-hint");
  expect(document.getElementById("scope-review-scope-hint")?.textContent)
    .toContain("When off, only PRs with a review requested from this account");
  control.focus();
  await user.keyboard(" ");
  expect(changed).toHaveBeenLastCalledWith({review_all_open_pull_requests:true});
  view.rerender(<SourceConnectorDialog id="scope"
    connector={{...source,review_all_open_pull_requests:true}}
    mode="existing" busy={false} attempted={false} onApply={vi.fn()}
    onChange={changed} onCancel={vi.fn()} returnFocus={()=>null}/>);
  expect(control.checked).toBe(true);
  await user.click(control);
  expect(changed).toHaveBeenLastCalledWith({review_all_open_pull_requests:false});
});

it("renders missing review scope as unchecked and disables it while busy", async () => {
  const {review_all_open_pull_requests: _scope, ...legacy} = source;
  const changed = vi.fn();
  render(<SourceConnectorDialog id="legacy-scope" connector={legacy}
    mode="existing" busy attempted={false} onApply={vi.fn()} onChange={changed}
    onCancel={vi.fn()} returnFocus={()=>null}/>);
  const control = screen.getByRole("checkbox", {
    name:"Review all open PRs in configured owners"
  }) as HTMLInputElement;
  expect(control.checked).toBe(false);
  expect(control.matches(":disabled")).toBe(true);
  await userEvent.setup().click(control);
  expect(changed).not.toHaveBeenCalled();
});

it("edits PR authors by keyboard and cancels without changing saved source", async () => {
  const user = userEvent.setup();
  const settings = {...noticeSettings(), source_connectors:[{
    ...source, review_all_open_pull_requests:true, pull_request_authors:["first"]
  }]};
  const onSave = vi.fn();
  render(<SourcesPage settings={settings} onSave={onSave}/>);
  open();
  const input = screen.getByLabelText("Allowed PR authors") as HTMLInputElement;
  expect(screen.getByText(/Empty includes all authors in configured owners/)).toBeTruthy();
  await user.click(input);
  await user.clear(input);
  await user.type(input, "first, second");
  expect(input.value).toBe("first, second");
  await user.keyboard("{Escape}");
  open();
  expect((screen.getByLabelText("Allowed PR authors") as HTMLInputElement).value).toBe("first");
  expect(onSave).not.toHaveBeenCalled();
});
