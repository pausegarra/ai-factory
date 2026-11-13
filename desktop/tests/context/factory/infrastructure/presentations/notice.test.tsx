import { useRef, useState } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { Notice } from "../../../../../src/context/factory/infrastructure/presentations/ui";

afterEach(cleanup);

function DismissHarness({
  onDismiss = vi.fn(),
  onSubmit = vi.fn(),
}: {
  onDismiss?: () => void;
  onSubmit?: () => void;
}) {
  const [visible, setVisible] = useState(true);
  const heading = useRef<HTMLHeadingElement>(null);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit();
      }}
    >
      <h2 ref={heading} tabIndex={-1}>
        Operation details
      </h2>
      <button type="button">Independent control</button>
      {visible && (
        <Notice
          close={() => {
            onDismiss();
            setVisible(false);
          }}
          focusAfterClose={() => heading.current}
        >
          Could not finish operation.
        </Notice>
      )}
    </form>
  );
}

it.each(["{Enter}", " "])(
  "dismisses using %s and restores focus to the local heading",
  async (key) => {
    const user = userEvent.setup();
    const onDismiss = vi.fn();
    render(<DismissHarness onDismiss={onDismiss} />);
    const close = screen.getByRole("button", { name: "Dismiss notice" });
    close.focus();
    await user.keyboard(key);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(
      screen.getByRole("heading", { name: "Operation details" }),
    );
  },
);

it("dismisses by pointer without submitting its form", async () => {
  const user = userEvent.setup();
  const onDismiss = vi.fn();
  const onSubmit = vi.fn();
  render(<DismissHarness onDismiss={onDismiss} onSubmit={onSubmit} />);
  await user.click(screen.getByRole("button", { name: "Dismiss notice" }));
  expect(screen.queryByRole("alert")).toBeNull();
  expect(onDismiss).toHaveBeenCalledTimes(1);
  expect(onSubmit).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Independent control" })).toBeTruthy();
});

it("uses alert, status, and static semantics for each notice tone", () => {
  const { rerender, container } = render(<Notice>Error message</Notice>);
  expect(screen.getByRole("alert").textContent).toContain("Error message");
  rerender(<Notice tone="success">Saved</Notice>);
  expect(screen.getByRole("status").textContent).toContain("Saved");
  rerender(<Notice tone="info">Information</Notice>);
  expect(screen.getByRole("status").textContent).toContain("Information");
  rerender(
    <Notice tone="info" live={false} id="delete-reason">
      Static help
    </Notice>,
  );
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByRole("status")).toBeNull();
  expect(container.querySelector("#delete-reason")).toBeTruthy();
  for (const icon of container.querySelectorAll("svg")) {
    expect(icon.getAttribute("aria-hidden")).toBe("true");
  }
});

it("does not move focus when the notice appears away from the close control", async () => {
  const user = userEvent.setup();
  function AppearingNotice() {
    const [visible, setVisible] = useState(false);
    return (
      <>
        <button type="button" onClick={() => setVisible(true)}>
          Show notice
        </button>
        {visible && <Notice>Could not finish operation.</Notice>}
      </>
    );
  }
  render(<AppearingNotice />);
  const show = screen.getByRole("button", { name: "Show notice" });
  show.focus();
  await user.click(show);
  await screen.findByRole("alert");
  expect(document.activeElement).toBe(show);
});

it("renders HTML-like content as inert text and keeps long diagnostics", () => {
  const payload = '<script>window.noticeInjected = true</script><img src=x onerror=alert(1)>';
  const long = "a".repeat(4096);
  const { container } = render(
    <Notice>{`${payload}\n${long}`}</Notice>,
  );
  const alert = screen.getByRole("alert");
  expect(alert.textContent).toContain(payload);
  expect(alert.textContent).toContain(long);
  expect(container.querySelector("script, img, a")).toBeNull();
});
