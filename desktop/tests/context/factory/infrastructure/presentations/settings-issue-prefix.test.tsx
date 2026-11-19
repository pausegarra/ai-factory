import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { Settings } from "../../../../../src/context/factory/domain/types";
import { SettingsPage } from "../../../../../src/context/factory/infrastructure/presentations/settings";
import { deferred, noticeSettings } from "../../../../fixtures/notices";

afterEach(cleanup);
const input = () => screen.getByRole("textbox", { name: "Issue title prefix" }) as HTMLInputElement;

it("shows the legacy default without making the form dirty", () => {
  const settings = noticeSettings();
  delete settings.issue_title_prefix;
  render(<SettingsPage settings={settings} onSave={vi.fn()} appDir="/fixture" />);
  expect(input().value).toBe("[AI]");
  expect((screen.getByRole("button", { name: "Save changes" }) as HTMLButtonElement).disabled).toBe(true);
});

it("saves a trimmed prefix and keeps the canonical value after reopen", async () => {
  const saved = vi.fn();
  function Harness() {
    const [settings, setSettings] = useState<Settings>(noticeSettings());
    return <SettingsPage settings={settings} appDir="/fixture" onSave={async next => {
      saved(next);
      const stored = { ...next, issue_title_prefix: next.issue_title_prefix!.trim() };
      setSettings(stored);
      return stored;
    }} />;
  }
  const view = render(<Harness />);
  fireEvent.change(input(), { target: { value: " \tBot: " } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
  expect(saved.mock.calls[0][0].issue_title_prefix).toBe("Bot:");
  await waitFor(() => expect(input().value).toBe("Bot:"));
  const persisted = saved.mock.calls[0][0] as Settings;
  view.unmount();
  render(<SettingsPage settings={persisted} onSave={vi.fn()} appDir="/fixture" />);
  expect(input().value).toBe("Bot:");
});

it.each(["", " \t\n"])("rejects an empty prefix %j without saving", async value => {
  const save = vi.fn();
  render(<SettingsPage settings={{ ...noticeSettings(), issue_title_prefix: "bot:" }}
    onSave={save} appDir="/fixture" />);
  fireEvent.change(input(), { target: { value } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Issue title prefix cannot be empty.");
  expect(save).not.toHaveBeenCalled();
});

it("provides a labelled, focusable field with help in General", () => {
  render(<SettingsPage settings={noticeSettings()} onSave={vi.fn()} appDir="/fixture" />);
  const field = within(screen.getByRole("tabpanel", { name: "General" })).getByRole("textbox", { name: "Issue title prefix" });
  const hint = document.getElementById(field.getAttribute("aria-describedby")!);
  expect(hint?.textContent).toContain("all eligible GitHub Sources");
  expect(hint?.textContent).toContain("Letter case matters");
  field.focus();
  expect(document.activeElement).toBe(field);
});

it("keeps a failed save editable and can discard to the persisted prefix", async () => {
  const dirty = vi.fn();
  render(<SettingsPage settings={{ ...noticeSettings(), issue_title_prefix: "[AI]" }} onSave={async () => { throw new Error("fixture save denied"); }} appDir="/fixture" onDirtyChange={dirty} />);
  fireEvent.change(input(), { target: { value: "bot:" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect((await screen.findByRole("alert")).textContent).toContain("fixture save denied");
  expect(input().value).toBe("bot:");
  expect(dirty).toHaveBeenLastCalledWith(true);
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect(input().value).toBe("[AI]");
  expect(dirty).toHaveBeenLastCalledWith(false);
});

it("refreshes a clean draft but preserves edits across settings updates", () => {
  const props = { onSave: vi.fn(), appDir: "/fixture" };
  const view = render(<SettingsPage {...props} settings={noticeSettings()} />);
  view.rerender(<SettingsPage {...props} settings={{ ...noticeSettings(), issue_title_prefix: "bot:" }} />);
  expect(input().value).toBe("bot:");
  fireEvent.change(input(), { target: { value: "BOT:" } });
  view.rerender(<SettingsPage {...props} settings={{ ...noticeSettings(), issue_title_prefix: "other:" }} />);
  expect(input().value).toBe("BOT:");
});

it("preserves a newer edit when an earlier save completes", async () => {
  const pending = deferred<Settings>();
  const dirty = vi.fn();
  const save = vi.fn(() => pending.promise);
  render(<SettingsPage settings={noticeSettings()} onSave={save} appDir="/fixture" onDirtyChange={dirty} />);
  fireEvent.change(input(), { target: { value: "bot:" } });
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  fireEvent.change(input(), { target: { value: "BOT:" } });
  await act(async () => pending.resolve({ ...noticeSettings(), issue_title_prefix: "bot:" }));
  expect(input().value).toBe("BOT:");
  expect(screen.queryByText("Settings saved.")).toBeNull();
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect((screen.getByRole("button", { name: "Save changes" }) as HTMLButtonElement).disabled).toBe(false);
});
