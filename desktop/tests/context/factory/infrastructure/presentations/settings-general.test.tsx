import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { newGitHubIssuesSource } from "../../../../../src/context/factory/domain/source-connectors";
import type { Settings } from "../../../../../src/context/factory/domain/types";
import { SettingsPage } from "../../../../../src/context/factory/infrastructure/presentations/settings";
import { deferred, noticeSettings } from "../../../../fixtures/notices";

afterEach(cleanup);
const interval = () => screen.getByRole<HTMLInputElement>("spinbutton", {name: /GitHub poll interval/});
const saveButton = () => screen.getByRole<HTMLButtonElement>("button", {name: "Save changes"});

it("keeps source title prefixes out of General and preserves the latest source values on save", async () => {
  const source = {...newGitHubIssuesSource("work"), account: "alice", has_token: true};
  const settings = {...noticeSettings(), source_connectors: [source]};
  const onSave = vi.fn(async (_settings: Settings) => {});
  const props = {onSave, appDir: "/fixture"};
  const view = render(<SettingsPage {...props} settings={settings} />);
  expect(within(screen.getByRole("tabpanel", {name: "General"}))
    .queryAllByRole("textbox", {name: /title prefix/i})).toEqual([]);
  fireEvent.change(interval(), {target: {value: "30"}});
  const incoming = {...source, issue_title_prefix: "Work:", release_issue_title_prefix: "Ship:"};
  view.rerender(<SettingsPage {...props} settings={{...settings, source_connectors: [incoming]}} />);
  expect(interval().value).toBe("30");
  fireEvent.click(saveButton());
  await screen.findByText("Settings saved.");
  const saved = onSave.mock.calls[0][0];
  expect(saved.source_connectors).toEqual([incoming]);
  expect(saved).not.toHaveProperty("issue_title_prefix");
  expect(saved).not.toHaveProperty("release_issue_title_prefix");
});

it("keeps a failed General save editable and can discard changes", async () => {
  const dirty = vi.fn();
  render(<SettingsPage settings={noticeSettings()} appDir="/fixture" onDirtyChange={dirty}
    onSave={async () => { throw new Error("fixture save denied"); }} />);
  fireEvent.change(interval(), {target: {value: "30"}});
  fireEvent.click(saveButton());
  expect((await screen.findByRole("alert")).textContent).toContain("fixture save denied");
  expect(interval().value).toBe("30");
  expect(dirty).toHaveBeenLastCalledWith(true);
  fireEvent.click(screen.getByRole("button", {name: "Discard"}));
  expect(interval().value).toBe("15");
  expect(dirty).toHaveBeenLastCalledWith(false);
});

it("refreshes a clean General draft and preserves unsaved edits on settings updates", () => {
  const props = {onSave: vi.fn(), appDir: "/fixture"};
  const view = render(<SettingsPage {...props} settings={noticeSettings()} />);
  view.rerender(<SettingsPage {...props} settings={{...noticeSettings(), poll_interval_seconds: 30}} />);
  expect(interval().value).toBe("30");
  fireEvent.change(interval(), {target: {value: "45"}});
  view.rerender(<SettingsPage {...props} settings={{...noticeSettings(), poll_interval_seconds: 60}} />);
  expect(interval().value).toBe("45");
});

it("preserves a newer General edit after an earlier save completes", async () => {
  const pending = deferred<Settings>();
  const dirty = vi.fn();
  const save = vi.fn(() => pending.promise);
  render(<SettingsPage settings={noticeSettings()} onSave={save} appDir="/fixture" onDirtyChange={dirty} />);
  fireEvent.change(interval(), {target: {value: "30"}});
  fireEvent.click(saveButton());
  fireEvent.change(interval(), {target: {value: "45"}});
  await act(async () => pending.resolve({...noticeSettings(), poll_interval_seconds: 30}));
  expect(interval().value).toBe("45");
  expect(screen.queryByText("Settings saved.")).toBeNull();
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect(saveButton().disabled).toBe(false);
});
