import { StrictMode, useRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ConnectorPicker, type ConnectorKind } from "../../../../../src/context/factory/infrastructure/presentations/connector-picker";
afterEach(cleanup);

function Harness({ disabled = false, onSelect = () => {} }: {
  disabled?: boolean;
  onSelect?: (kind: ConnectorKind) => void;
}) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  return <main tabIndex={-1}>
    <button>Outside</button>
    <ConnectorPicker disabled={disabled} buttonRef={buttonRef} onSelect={onSelect} />
  </main>;
}

it("opens an accessible modal with exactly two named provider actions", async () => {
  const user = userEvent.setup();
  const selected = vi.fn();
  render(<StrictMode><Harness onSelect={selected} /></StrictMode>);
  const add = screen.getByRole("button", { name: "Add connector" });
  add.focus();
  await user.keyboard("{Enter}");
  const dialog = screen.getByRole("dialog", { name: "Choose a connector" });
  expect(dialog.getAttribute("aria-modal")).toBe("true");
  expect(add.getAttribute("aria-haspopup")).toBe("dialog");
  expect(add.getAttribute("aria-expanded")).toBe("true");
  expect(document.getElementById(add.getAttribute("aria-controls")!)).toBe(dialog);
  expect(screen.queryByRole("combobox", { name: "Connector to add" })).toBeNull();
  const close = within(dialog).getByRole("button", { name: "Close connector selector" });
  const telegram = within(dialog).getByRole("button", { name: "Telegram" });
  const chat = within(dialog).getByRole("button", { name: "Google Chat" });
  expect(within(dialog).getAllByRole("button")).toEqual([close, telegram, chat]);
  expect(document.activeElement).toBe(close);
  for (const option of [telegram, chat]) {
    const icon = option.querySelector("svg, img")!;
    expect(icon).toBeTruthy();
    expect(icon.getAttribute("aria-hidden")).toBe("true");
  }
  expect(within(dialog).queryAllByRole("img")).toHaveLength(0);
  expect(selected).not.toHaveBeenCalled();
});

it.each(["telegram", "google_chat"] as const)("selects %s exactly once by keyboard", async kind => {
  const user = userEvent.setup();
  const selected = vi.fn();
  render(<Harness onSelect={selected} />);
  await user.click(screen.getByRole("button", { name: "Add connector" }));
  const dialog = screen.getByRole("dialog");
  const option = within(dialog).getByRole("button", {
    name: kind === "telegram" ? "Telegram" : "Google Chat",
  });
  option.focus();
  await user.keyboard(kind === "telegram" ? "{Enter}" : " ");
  expect(selected).toHaveBeenCalledExactlyOnceWith(kind);
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("contains both tab directions and redirects external focus only while open", async () => {
  const user = userEvent.setup();
  render(<Harness />);
  await user.click(screen.getByRole("button", { name: "Add connector" }));
  const dialog = screen.getByRole("dialog");
  const close = within(dialog).getByRole("button", { name: "Close connector selector" });
  const telegram = within(dialog).getByRole("button", { name: "Telegram" });
  const chat = within(dialog).getByRole("button", { name: "Google Chat" });
  await user.tab({ shift: true }); expect(document.activeElement).toBe(chat);
  await user.tab(); expect(document.activeElement).toBe(close);
  await user.tab(); expect(document.activeElement).toBe(telegram);
  await user.tab(); expect(document.activeElement).toBe(chat);
  await user.tab(); expect(document.activeElement).toBe(close);
  const outside = screen.getByRole("button", { name: "Outside" });
  outside.focus(); expect(document.activeElement).toBe(close);
  await user.keyboard("{Escape}");
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add connector" })));
  outside.focus(); expect(document.activeElement).toBe(outside);
});

it.each(["escape", "close"])("cancels through %s without selection and restores focus", async method => {
  const user = userEvent.setup(); const selected = vi.fn();
  render(<StrictMode><Harness onSelect={selected} /></StrictMode>);
  const add = screen.getByRole("button", { name: "Add connector" });
  await user.click(add);
  if (method === "escape") await user.keyboard("{Escape}");
  else await user.click(screen.getByRole("button", { name: "Close connector selector" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  await waitFor(() => expect(document.activeElement).toBe(add));
  expect(add.getAttribute("aria-expanded")).toBe("false");
  expect(add.getAttribute("aria-controls")).toBeNull();
  expect(selected).not.toHaveBeenCalled();
});

it("closes on disabled transition, uses fallback focus and never reopens implicitly", async () => {
  const user = userEvent.setup(); const selected = vi.fn();
  const view = render(<Harness onSelect={selected} />);
  await user.click(screen.getByRole("button", { name: "Add connector" }));
  view.rerender(<Harness disabled onSelect={selected} />);
  expect(screen.queryByRole("dialog")).toBeNull();
  const add = screen.getByRole("button", { name: "Add connector" });
  expect(add.matches(":disabled")).toBe(true);
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("main")));
  await user.click(add); expect(screen.queryByRole("dialog")).toBeNull();
  view.rerender(<Harness onSelect={selected} />);
  expect(screen.queryByRole("dialog")).toBeNull();
  await user.click(add); expect(screen.getByRole("dialog")).toBeTruthy();
  expect(selected).not.toHaveBeenCalled();
});

it("restores a safe fallback when the opener unmounts and removes its focus listener", async () => {
  const user = userEvent.setup();
  const fallback = document.createElement("main"); fallback.tabIndex = -1;
  document.body.append(fallback);
  const view = render(<Harness />);
  try {
    await user.click(screen.getByRole("button", { name: "Add connector" }));
    view.unmount();
    await waitFor(() => expect(document.activeElement).toBe(fallback));
    const outside = document.createElement("button");
    document.body.append(outside);
    outside.focus(); expect(document.activeElement).toBe(outside);
    outside.remove();
  } finally { view.unmount(); fallback.remove(); }
});
