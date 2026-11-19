import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { Settings } from "../../../../../src/context/factory/domain/types";
import { SettingsPage } from "../../../../../src/context/factory/infrastructure/presentations/settings";
import { deferred, noticeSettings } from "../../../../fixtures/notices";

afterEach(cleanup);
const input = () => screen.getByRole("textbox", { name: "Release issue title prefix" }) as HTMLInputElement;

it("shows the legacy default without dirtying the form", () => {
  render(<SettingsPage settings={noticeSettings()} onSave={vi.fn()} appDir="/fixture" />);
  expect(input().value).toBe("[RELEASE]");
  expect((screen.getByRole("button", { name: "Save changes" }) as HTMLButtonElement).disabled).toBe(true);
});

it("saves the trimmed release prefix independently and keeps it after reopen", async () => {
  const saved = vi.fn();
  function Harness() {
    const [settings, setSettings] = useState<Settings>({ ...noticeSettings(), issue_title_prefix: "task:" });
    return <SettingsPage settings={settings} appDir="/fixture" onSave={async next => {
      saved(next);
      setSettings(next);
      return next;
    }} />;
  }
  const view = render(<Harness />);
  fireEvent.change(input(), { target: { value: " \tShip: " } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
  expect(saved.mock.calls[0][0].release_issue_title_prefix).toBe("Ship:");
  expect(saved.mock.calls[0][0].issue_title_prefix).toBe("task:");
  await waitFor(() => expect(input().value).toBe("Ship:"));
  const stored = saved.mock.calls[0][0] as Settings;
  view.unmount();
  render(<SettingsPage settings={stored} onSave={vi.fn()} appDir="/fixture" />);
  expect(input().value).toBe("Ship:");
});

it.each(["", " \t\n"])("rejects an empty Release prefix %j without saving", async value => {
  const save = vi.fn();
  render(<SettingsPage settings={{ ...noticeSettings(), release_issue_title_prefix: "ship:" }} onSave={save} appDir="/fixture" />);
  fireEvent.change(input(), { target: { value } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Release issue title prefix cannot be empty.");
  expect(save).not.toHaveBeenCalled();
});

it("provides a focusable labelled field and exact matching help in General", () => {
  render(<SettingsPage settings={noticeSettings()} onSave={vi.fn()} appDir="/fixture" />);
  const field = within(screen.getByRole("tabpanel", { name: "General" })).getByRole("textbox", { name: "Release issue title prefix" });
  const help = document.getElementById(field.getAttribute("aria-describedby")!);
  expect(help?.textContent).toContain("start with this exact text");
  expect(help?.textContent).toContain("trimming outer whitespace");
  expect(help?.textContent).toContain("Letter case matters");
  expect(help?.textContent).toContain("Release across all eligible GitHub Sources");
  field.focus();
  expect(document.activeElement).toBe(field);
});

it("keeps failed saves editable and discards to the saved prefix", async () => {
  const dirty = vi.fn();
  render(<SettingsPage settings={{ ...noticeSettings(), release_issue_title_prefix: "ship:" }} appDir="/fixture" onDirtyChange={dirty}
    onSave={async () => { throw new Error("fixture save denied"); }} />);
  fireEvent.change(input(), { target: { value: "deploy:" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect((await screen.findByRole("alert")).textContent).toContain("fixture save denied");
  expect(input().value).toBe("deploy:");
  expect(dirty).toHaveBeenLastCalledWith(true);
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect(input().value).toBe("ship:");
  expect(dirty).toHaveBeenLastCalledWith(false);
});

it("refreshes a clean draft and preserves unsaved edits on settings updates", () => {
  const props = { onSave: vi.fn(), appDir: "/fixture" };
  const view = render(<SettingsPage {...props} settings={noticeSettings()} />);
  view.rerender(<SettingsPage {...props} settings={{ ...noticeSettings(), release_issue_title_prefix: "ship:" }} />);
  expect(input().value).toBe("ship:");
  fireEvent.change(input(), { target: { value: "deploy:" } });
  view.rerender(<SettingsPage {...props} settings={{ ...noticeSettings(), release_issue_title_prefix: "other:" }} />);
  expect(input().value).toBe("deploy:");
});

it("preserves a newer edit after an earlier save completes", async () => {
  const pending = deferred<Settings>();
  const dirty = vi.fn();
  const save = vi.fn(() => pending.promise);
  render(<SettingsPage settings={noticeSettings()} onSave={save} appDir="/fixture" onDirtyChange={dirty} />);
  fireEvent.change(input(), { target: { value: "ship:" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  fireEvent.change(input(), { target: { value: "deploy:" } });
  await act(async () => pending.resolve({ ...noticeSettings(), release_issue_title_prefix: "ship:" }));
  expect(input().value).toBe("deploy:");
  expect(screen.queryByText("Settings saved.")).toBeNull();
  expect(dirty).toHaveBeenLastCalledWith(true);
});
