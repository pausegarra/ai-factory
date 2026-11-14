import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Settings } from "../../../../../src/context/factory/domain/types";
import { SettingsPage } from "../../../../../src/context/factory/infrastructure/presentations/settings";
import { noticeSettings } from "../../../../fixtures/notices";

afterEach(cleanup);
const checkbox = () => screen.getByRole("checkbox", { name: /^Automatic issue comments/ }) as HTMLInputElement;
const saveButton = () => screen.getByRole("button", { name: "Save changes" }) as HTMLButtonElement;

it("defaults old settings to enabled in General and is keyboard accessible", async () => {
  const value = noticeSettings();
  delete value.issue_node_comments_enabled;
  render(<SettingsPage settings={value} onSave={vi.fn()} appDir="/fixture" />);
  const general = screen.getByRole("tabpanel", { name: "General" });
  expect(within(general).getByRole("checkbox", { name: /^Automatic issue comments/ })).toBe(checkbox());
  expect(checkbox().checked).toBe(true);
  expect(saveButton().disabled).toBe(true);
  expect(screen.getByText(/for all accounts in this installation/)).toBeTruthy();
  checkbox().focus();
  expect(document.activeElement).toBe(checkbox());
  await userEvent.setup().keyboard(" ");
  expect(checkbox().checked).toBe(false);
  expect(saveButton().disabled).toBe(false);
});

it("saves both values through the parent and discards to the latest saved value", async () => {
  const dirty = vi.fn();
  const saved = vi.fn();
  function Harness() {
    const [settings, setSettings] = useState<Settings>({ ...noticeSettings(), issue_node_comments_enabled: true });
    return <SettingsPage settings={settings} appDir="/fixture" onDirtyChange={dirty} onSave={async next => {
      saved(next);
      setSettings(next);
      return next;
    }} />;
  }
  render(<Harness />);
  fireEvent.click(checkbox());
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect(saved).not.toHaveBeenCalled();
  fireEvent.click(saveButton());
  await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
  expect(saved.mock.calls[0][0].issue_node_comments_enabled).toBe(false);
  await waitFor(() => expect(saveButton().disabled).toBe(true));
  expect(dirty).toHaveBeenLastCalledWith(false);
  fireEvent.click(checkbox());
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  expect(checkbox().checked).toBe(false);
  expect(dirty).toHaveBeenLastCalledWith(false);
  fireEvent.click(checkbox());
  fireEvent.click(saveButton());
  await waitFor(() => expect(saved).toHaveBeenCalledTimes(2));
  expect(saved.mock.calls[1][0].issue_node_comments_enabled).toBe(true);
  await waitFor(() => expect(saveButton().disabled).toBe(true));
  expect(checkbox().checked).toBe(true);
});

it("keeps the unsaved false value and dirty state after a save error", async () => {
  const dirty = vi.fn();
  const save = vi.fn().mockRejectedValue(new Error("fixture save denied"));
  render(<SettingsPage settings={{ ...noticeSettings(), issue_node_comments_enabled: true }} onSave={save} onDirtyChange={dirty} appDir="/fixture" />);
  fireEvent.click(checkbox());
  fireEvent.click(saveButton());
  expect((await screen.findByRole("alert")).textContent).toContain("fixture save denied");
  expect(save.mock.calls[0][0].issue_node_comments_enabled).toBe(false);
  expect(checkbox().checked).toBe(false);
  expect(dirty).toHaveBeenLastCalledWith(true);
  expect(saveButton().disabled).toBe(false);
});

it("reflects saved false values from settings events without overwriting a dirty draft", () => {
  const value = { ...noticeSettings(), issue_node_comments_enabled: true };
  const view = render(<SettingsPage settings={value} onSave={vi.fn()} appDir="/fixture" />);
  const latest = { ...value, issue_node_comments_enabled: false };
  view.rerender(<SettingsPage settings={latest} onSave={vi.fn()} appDir="/fixture" />);
  expect(checkbox().checked).toBe(false);
  fireEvent.click(checkbox());
  view.rerender(<SettingsPage settings={{ ...latest, poll_interval_seconds: 30 }} onSave={vi.fn()} appDir="/fixture" />);
  expect(checkbox().checked).toBe(true);
  expect(saveButton().disabled).toBe(false);
});
