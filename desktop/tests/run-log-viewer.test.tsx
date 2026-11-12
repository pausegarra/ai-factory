import { afterEach, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { RunLogProjection } from "../src/context/factory/application/services/run-log-projection";
import type { LogDocument, LogViewMode } from "../src/context/factory/application/dto/run-log";
import { RunLogViewer } from "../src/context/factory/infrastructure/presentations/run-log-viewer";
import { command, envelope, jsonLine, openText, tool } from "./fixtures/run-logs";

afterEach(cleanup);

const documentOf = (raw: string): LogDocument =>
  new RunLogProjection().update({ text: raw, eof: true });

function Harness({ document: logDocument, follow = true }: { document: LogDocument; follow?: boolean }) {
  const [mode, setMode] = useState<LogViewMode>("readable");
  return <RunLogViewer document={logDocument} mode={mode} onModeChange={setMode} follow={follow} />;
}

it("expands blocks independently and retains state across Raw and append", async () => {
  const user = userEvent.setup();
  const raw = jsonLine(openText("a".repeat(1201))) + "\n" + jsonLine(openText("b".repeat(1201))) + "\n";
  const { rerender } = render(<Harness document={documentOf(raw)} />);
  const [first, second] = screen.getAllByRole("button", { name: /Show more/ });
  await user.click(first);
  expect(first.getAttribute("aria-expanded")).toBe("true");
  expect(second.getAttribute("aria-expanded")).toBe("false");
  first.focus();
  await user.keyboard(" ");
  expect(first.getAttribute("aria-expanded")).toBe("false");
  await user.keyboard("{Enter}");
  expect(globalThis.document.activeElement).toBe(first);
  await user.click(screen.getByRole("button", { name: "Raw" }));
  expect(screen.getByTestId("raw-log-output").textContent).toBe(raw);
  expect(screen.queryByRole("button", { name: /Show less/ })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Readable" }));
  expect(screen.getAllByRole("button", { name: /Show less/ })).toHaveLength(1);
  rerender(<Harness document={documentOf(raw + "extra\n")} />);
  expect(screen.getAllByRole("button", { name: /Show less/ })).toHaveLength(1);
  expect(screen.getByText("extra")).toBeTruthy();
});

it("starts result details closed, separately expands long detail and keeps it across modes", async () => {
  const user = userEvent.setup();
  const raw = jsonLine(openText(jsonLine({ ...envelope, data: { note: "z".repeat(2000) } })));
  render(<Harness document={documentOf(raw)} />);
  const detail = screen.getByRole("button", { name: "Result details" });
  expect(detail.getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByText("Reported status: succeeded")).toBeTruthy();
  await user.click(detail);
  expect(screen.getByRole("button", { name: /Show more/ })).toBeTruthy();
  expect(globalThis.document.activeElement).toBe(detail);
  await user.click(screen.getByRole("button", { name: "Raw" }));
  await user.click(screen.getByRole("button", { name: "Readable" }));
  expect(screen.getByRole("button", { name: "Result details" }).getAttribute("aria-expanded"))
    .toBe("true");
});

it("renders all block categories as inert text without mass live announcements", () => {
  const payload = '<script>window.evil=1</script> ![image](https://invalid.test/a) [link](https://invalid.test) [REDACTED]';
  const raw = [
    jsonLine(openText(payload)),
    jsonLine(command),
    jsonLine(tool),
    jsonLine({ type: "error", message: "denied" }),
    "plain error",
  ].join("\n");
  const { container } = render(<Harness document={documentOf(raw)} />);
  expect(screen.getByText(payload)).toBeTruthy();
  expect(screen.getByText("denied")).toBeTruthy();
  expect(screen.getByText("plain error")).toBeTruthy();
  expect(container.querySelector("script, img, a, [aria-live], [role='log'], [role='alert']")).toBeNull();
  expect(screen.getByRole("button", { name: "Readable" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getAllByText("Command").length).toBeGreaterThan(0);
});

it("shows empty message and complete 256 KiB block on request", async () => {
  const user = userEvent.setup();
  const { rerender } = render(<Harness document={documentOf("")} />);
  expect(screen.getByText("Waiting for agent output…")).toBeTruthy();
  const text = "z".repeat(256 * 1024);
  rerender(<Harness document={documentOf(jsonLine(openText(text)))} />);
  const button = screen.getByRole("button", { name: /Show more/ });
  const contentId = button.getAttribute("aria-controls")!;
  expect(globalThis.document.getElementById(contentId)?.textContent).toBe(text.slice(0, 1200));
  await user.click(button);
  expect(globalThis.document.getElementById(contentId)?.textContent).toBe(text);
});

it("supports native Tab, Enter and Space on the mode selector", async () => {
  const user = userEvent.setup();
  render(<Harness document={documentOf("plain\n")} />);
  const readable = screen.getByRole("button", { name: "Readable" });
  const raw = screen.getByRole("button", { name: "Raw" });
  readable.focus();
  await user.tab();
  expect(globalThis.document.activeElement).toBe(raw);
  await user.keyboard("{Enter}");
  expect(raw.getAttribute("aria-pressed")).toBe("true");
  expect(globalThis.document.activeElement).toBe(raw);
  await user.tab({ shift: true });
  await user.keyboard(" ");
  expect(readable.getAttribute("aria-pressed")).toBe("true");
  expect(globalThis.document.activeElement).toBe(readable);
});

it("follows appended text, never folding, mode toggles or unchanged refreshes", async () => {
  const user = userEvent.setup();
  const raw = jsonLine(openText("a".repeat(1201))) + "\n";
  const { rerender } = render(<Harness document={documentOf(raw)} />);
  const viewport = screen.getByRole("region", { name: "Session output" });
  Object.defineProperty(viewport, "scrollHeight", { configurable: true, get: () => 2000 });
  Object.defineProperty(viewport, "clientHeight", { configurable: true, get: () => 400 });
  (viewport as HTMLDivElement).scrollTop = 70;
  viewport.dispatchEvent(new Event("scroll", { bubbles: true }));
  await user.click(screen.getByRole("button", { name: /Show more/ }));
  expect((viewport as HTMLDivElement).scrollTop).toBe(70);
  await user.click(screen.getByRole("button", { name: "Raw" }));
  expect((viewport as HTMLDivElement).scrollTop).toBe(70);
  expect(globalThis.document.activeElement).toBe(screen.getByRole("button", { name: "Raw" }));
  rerender(<Harness document={documentOf(raw + "next\n")} />);
  expect((viewport as HTMLDivElement).scrollTop).toBe(2000);
  (viewport as HTMLDivElement).scrollTop = 80;
  viewport.dispatchEvent(new Event("scroll", { bubbles: true }));
  rerender(<Harness follow={false} document={documentOf(raw + "next\nlast\n")} />);
  expect((viewport as HTMLDivElement).scrollTop).toBe(80);
  rerender(<Harness follow={false} document={documentOf(raw + "next\nlast\n")} />);
  expect((viewport as HTMLDivElement).scrollTop).toBe(80);
});

it("renders 1000 records without loss or one expansion control per short entry", async () => {
  const user = userEvent.setup();
  const long = "z".repeat(256 * 1024);
  const raw = Array.from({ length: 1000 }, (_, index) => jsonLine(openText(`entry ${index}`))).join("\n") +
    "\n" + jsonLine(openText(long));
  const { container, rerender } = render(<Harness document={documentOf(raw)} />);
  expect(container.querySelectorAll("[data-entry-id]")).toHaveLength(1001);
  expect(screen.getByText("entry 999")).toBeTruthy();
  expect(screen.getAllByRole("button", { name: /Show more/ })).toHaveLength(1);
  const more = screen.getByRole("button", { name: /Show more/ });
  expect(globalThis.document.getElementById(more.getAttribute("aria-controls")!)).toBeTruthy();
  await user.click(more);
  expect(globalThis.document.getElementById(more.getAttribute("aria-controls")!)?.textContent).toBe(long);
  more.focus();
  rerender(<Harness document={documentOf(raw + "\n")} />);
  expect(globalThis.document.activeElement).toBe(more);
  expect(screen.getAllByRole("button", { name: /Show less/ })).toHaveLength(1);
});

it.each(["codex", "opencode"])("folds each tool block independently for %s", async (agent) => {
  const user = userEvent.setup();
  const source = agent === "codex" ? {
    ...command,
    item: { ...command.item, command: "c".repeat(1201), aggregated_output: "o".repeat(1201) },
  } : {
    type: "tool_use",
    part: { type: "tool", tool: "read", state: { input: { path: "p".repeat(1201) }, output: "o".repeat(1201) } },
  };
  render(<Harness document={documentOf(jsonLine(source))} />);
  const first = screen.getByRole("button", {
    name: agent === "codex" ? "Show more Command" : "Show more Input",
  });
  const output = screen.getByRole("button", { name: "Show more Output" });
  await user.click(first);
  expect(first.getAttribute("aria-expanded")).toBe("true");
  expect(output.getAttribute("aria-expanded")).toBe("false");
  expect(globalThis.document.getElementById(output.getAttribute("aria-controls")!)?.textContent)
    .toBe("o".repeat(1200));
  await user.click(output);
  expect(globalThis.document.getElementById(output.getAttribute("aria-controls")!)?.textContent)
    .toBe("o".repeat(1201));
});
