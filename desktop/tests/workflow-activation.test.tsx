import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import type { Flow, TokenProfile } from "../src/context/factory/domain/types";
import { WorkflowActivationFields } from "../src/context/factory/infrastructure/presentations/workflow-activation-fields";

afterEach(cleanup);
const flow = (): Flow => ({ id: "new_issue", name: "Issue", enabled: false, nodes: [] });
const profiles: TokenProfile[] = [
  { id: "work", label: "Work", account: "alice", has_token: true },
  { id: "personal", label: "Personal", account: "bob", has_token: false },
];
it("edits account preferences while globally disabled without mutating input", () => {
  const value = flow();
  const change = vi.fn();
  render(<WorkflowActivationFields flow={value} profiles={profiles} onChange={change} />);
  const checkbox = screen.getByRole("checkbox", { name: "Enable new_issue for Work (@alice)" }) as HTMLInputElement;
  expect(checkbox.checked).toBe(true);
  expect(checkbox.disabled).toBe(false);
  expect(screen.getAllByRole("status").map((status) => status.textContent)).toEqual(["Disabled globally", "Disabled globally"]);
  expect((screen.getByRole("checkbox", { name: "Enable new_issue for Personal (@bob)" }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(checkbox);
  expect(change).toHaveBeenCalledWith({ ...value, profile_activation: { work: false } });
  expect(value.profile_activation).toBeUndefined();
  expect(screen.getByText(/No token saved/)).toBeTruthy();
  expect(screen.getByText(/Queued and running tasks continue/)).toBeTruthy();
  expect(screen.getByText(/Events no longer observable on GitHub are not replayed/)).toBeTruthy();
});
it("keeps global control and Settings help without accounts and removes orphan preferences", () => {
  const value = { ...flow(), profile_activation: { removed: false } };
  const change = vi.fn();
  render(<WorkflowActivationFields flow={value} profiles={[]} onChange={change} />);
  fireEvent.click(screen.getByRole("checkbox", { name: "Enable workflow globally for new_issue" }));
  expect(change).toHaveBeenCalledWith({ ...value, enabled: true });
  expect(screen.getByText(/Add GitHub accounts in Settings/)).toBeTruthy();
  expect(screen.getByRole("group", { name: "Unavailable activation profile: removed" })).toBeTruthy();
  expect(screen.getByText(/no effect until this profile ID exists/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Remove activation preference for removed" }));
  expect(change).toHaveBeenLastCalledWith({ ...value, profile_activation: {} });
  expect(value.profile_activation).toEqual({ removed: false });
});
it("supports keyboard checkbox activation and reports effective state", async () => {
  function Editor() {
    const [value, setValue] = useState(flow());
    return <WorkflowActivationFields flow={value} profiles={[profiles[0]]} onChange={setValue} />;
  }
  render(<Editor />);
  const user = userEvent.setup();
  await user.tab();
  expect(document.activeElement).toBe(screen.getByRole("checkbox", { name: "Enable workflow globally for new_issue" }));
  await user.keyboard(" ");
  expect(screen.getByRole("status").textContent).toBe("Enabled");
  await user.tab();
  expect(document.activeElement).toBe(screen.getByRole("checkbox", { name: "Enable new_issue for Work (@alice)" }));
  await user.keyboard(" ");
  expect(screen.getByRole("status").textContent).toBe("Disabled for this account");
});
it.each(["constructor", "__proto__"])("preserves exact own profile key %s during changes and orphan removal", (id) => {
  const value = { ...flow(), profile_activation: JSON.parse('{"constructor":false,"__proto__":false}') };
  const change = vi.fn();
  const profile = { ...profiles[0], id };
  const view = render(<WorkflowActivationFields flow={value} profiles={[profile]} onChange={change} />);
  fireEvent.click(screen.getByRole("checkbox", { name: "Enable new_issue for Work (@alice)" }));
  const next = change.mock.calls[0][0];
  expect(Object.keys(next.profile_activation)).toEqual(["constructor", "__proto__"]);
  expect(next.profile_activation[id]).toBe(true);
  expect(next.profile_activation[id === "constructor" ? "__proto__" : "constructor"]).toBe(false);
  view.rerender(<WorkflowActivationFields flow={next} profiles={[]} onChange={change} />);
  fireEvent.click(screen.getByRole("button", { name: `Remove activation preference for ${id}` }));
  expect(Object.hasOwn(change.mock.calls[1][0].profile_activation, id)).toBe(false);
});
