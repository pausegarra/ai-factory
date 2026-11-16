import { fireEvent, screen, within } from "@testing-library/react";

export function openConnector(label: string, occurrence = 0): HTMLElement {
  fireEvent.click(screen.getAllByRole("button", {name: `Edit connector ${label}`})[occurrence]);
  return screen.getByRole("dialog", {name: "Edit connector"});
}
export function chooseConnector(label: "Telegram" | "Google Chat"): HTMLElement {
  fireEvent.click(screen.getByRole("button", {name: "Add connector"}));
  fireEvent.click(within(screen.getByRole("dialog", {name: "Choose a connector"})).getByRole("button", {name: label}));
  return screen.getByRole("dialog", {name: "Configure new connector"});
}
export function applyConnector(): void {
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", {name: "Apply changes"}));
}
export function cancelConnector(): void {
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", {name: "Cancel"}));
}
