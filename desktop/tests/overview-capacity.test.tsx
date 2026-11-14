import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, it, expect, vi } from "vitest";
import stylesheet from "../src/styles.css?raw";
import { OverviewPage } from "../src/context/factory/infrastructure/presentations/overview";
import { attentionSettings, attentionSnapshot, attentionHistory } from "./fixtures/attention";
afterEach(cleanup);
it("uses reserved capacity independently of historical run states", () => {
  const value = attentionSnapshot(attentionHistory(["queued", "queued", "queued"]));
  value.capacity = { global: { active: 3, limit: 8 }, by_trigger: {
    new_issue: { active: 1, limit: 4 }, pr_review: { active: 0, limit: 3 }, pr_approved: { active: 2, limit: 2 },
  } };
  render(<OverviewPage snapshot={value} settings={attentionSettings} onPage={() => {}} onRun={() => {}} />);
  const panel = screen.getByRole("region", { name: "Capacity" });
  expect(within(panel).getByLabelText("Global capacity").textContent).toBe("3 / 8");
  expect(within(panel).getByLabelText("New issue capacity").textContent).toBe("1 / 4");
  expect(within(panel).getByLabelText("PR review capacity").textContent).toBe("0 / 3");
  expect(within(panel).getByLabelText("PR approved capacity").textContent).toBe("2 / 2");
  expect(within(panel).getByText("5 slots available for new tasks.")).toBeTruthy();
});
it("shows real occupancy above a reduced limit until workers drain", () => {
  const value = attentionSnapshot([]);
  value.capacity = { global: { active: 2, limit: 1 }, by_trigger: {
    new_issue: { active: 2, limit: 1 }, pr_review: { active: 0, limit: 1 }, pr_approved: { active: 0, limit: 1 },
  } };
  const props = { settings: attentionSettings, onPage: () => {}, onRun: () => {} };
  const view = render(<OverviewPage snapshot={value} {...props} />);
  expect(screen.getByLabelText("Global capacity").textContent).toBe("2 / 1");
  expect(screen.getByLabelText("New issue capacity").textContent).toBe("2 / 1");
  expect(screen.getByText("0 slots available for new tasks.")).toBeTruthy();
  view.rerender(<OverviewPage snapshot={attentionSnapshot(attentionHistory(["queued"]))} {...props} />);
  expect(screen.getByLabelText("Global capacity").textContent).toBe("0 / 4");
});

function rgb(color: string): number[] {
  const channels = color.match(/[\d.]+/g)?.slice(0, 3).map(Number);
  if (!channels || channels.length !== 3) {
    throw new Error("Expected a computed RGB color: " + color);
  }
  return channels;
}

function luminance(color: string): number {
  const channels = rgb(color).map((channel) => {
    const value = channel / 255;
    return value <= 0.04045
      ? value / 12.92
      : ((value + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(foreground: string, background: string): number {
  const a = luminance(foreground);
  const b = luminance(background);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

it("aligns trigger metrics with readable neutral secondary styling", () => {
  const style = document.createElement("style");
  style.textContent = stylesheet
    .replace(/^@import[^\n]*$/gm, "")
    .replace(/@theme\s*\{[^}]*\}/g, "");
  document.head.appendChild(style);
  try {
    expect(style.sheet).not.toBeNull();
    expect(style.sheet!.cssRules.length).toBeGreaterThan(0);
    const value = attentionSnapshot([]);
    value.capacity = {
      global: { active: 23, limit: 64 },
      by_trigger: {
        new_issue: { active: 2, limit: 1 },
        pr_review: { active: 10, limit: 32 },
        pr_approved: { active: 11, limit: 64 },
      },
    };
    render(
      <OverviewPage snapshot={value} settings={attentionSettings}
        onPage={() => {}} onRun={() => {}} />,
    );
    const panel = screen.getByRole("region", { name: "Capacity" });
    const list = panel.querySelector("dl");
    expect(list).not.toBeNull();
    const rows = Array.from(list!.children);
    expect(rows).toHaveLength(3);
    const panelStyle = getComputedStyle(panel);
    const globalNumber = panel.querySelector(".capacity-value strong")!;
    const globalSize = parseFloat(getComputedStyle(globalNumber).fontSize);
    const sizes: string[] = [];

    for (const [index, row] of rows.entries()) {
      const layout = getComputedStyle(row);
      expect(layout.display).toBe("grid");
      expect(layout.gridTemplateColumns).toMatch(/minmax\(0,\s*1fr\)/);
      expect(layout.gridTemplateColumns).toContain("max-content");
      expect(parseFloat(layout.paddingTop)).toBeGreaterThan(0);
      expect(parseFloat(layout.columnGap)).toBeGreaterThan(0);
      expect(["transparent", "rgba(0, 0, 0, 0)"]).toContain(layout.backgroundColor);
      expect(layout.cursor).not.toBe("pointer");
      if (index > 0) {
        expect(parseFloat(layout.borderTopWidth)).toBeGreaterThan(0);
        expect(layout.borderTopColor).toBe(panelStyle.borderTopColor);
      }
      const label = row.querySelector("dt")!;
      const valueElement = row.querySelector("dd")!;
      const labelStyle = getComputedStyle(label);
      const valueStyle = getComputedStyle(valueElement);
      expect(valueStyle.textAlign).toBe("right");
      expect(valueStyle.marginLeft).toBe("0px");
      expect(valueStyle.getPropertyValue("font-variant-numeric")).toContain("tabular-nums");
      expect(valueStyle.textOverflow).not.toBe("ellipsis");
      expect(parseFloat(labelStyle.fontSize)).toBeLessThanOrEqual(parseFloat(valueStyle.fontSize));
      expect(parseFloat(valueStyle.fontSize)).toBeLessThan(globalSize);
      sizes.push(valueStyle.fontSize);
      for (const textStyle of [labelStyle, valueStyle]) {
        expect(contrast(textStyle.color, panelStyle.backgroundColor)).toBeGreaterThanOrEqual(4.5);
        const channels = rgb(textStyle.color);
        expect(Math.max(...channels) - Math.min(...channels)).toBeLessThanOrEqual(8);
      }
    }
    expect(new Set(sizes).size).toBe(1);
    expect(within(list!).queryByRole("button")).toBeNull();
    expect(list!.querySelector("a, input, select, textarea, [tabindex]")).toBeNull();
    expect(screen.getByLabelText("New issue capacity").textContent).toBe("2 / 1");
    expect(screen.getByLabelText("PR review capacity").textContent).toBe("10 / 32");
    expect(screen.getByLabelText("PR approved capacity").textContent).toBe("11 / 64");
  } finally {
    style.remove();
  }
});

it("preserves trigger order and refreshes every reading without changing Settings navigation", async () => {
  const user = userEvent.setup();
  const onPage = vi.fn();
  const value = attentionSnapshot([]);
  value.capacity = {
    global: { active: 23, limit: 64 },
    by_trigger: {
      new_issue: { active: 2, limit: 1 },
      pr_review: { active: 10, limit: 32 },
      pr_approved: { active: 11, limit: 64 },
    },
  };
  const props = { settings: attentionSettings, onPage, onRun: () => {} };
  const view = render(<OverviewPage snapshot={value} {...props} />);
  expect(screen.getAllByRole("region", { name: "Capacity" })).toHaveLength(1);
  const panel = screen.getByRole("region", { name: "Capacity" });
  expect(Array.from(panel.querySelectorAll("dt"), (node) => node.textContent))
    .toEqual(["New issue", "PR review", "PR approved"]);
  expect(within(panel).getByLabelText("Global capacity").textContent).toBe("23 / 64");
  expect(within(panel).getByText("41 slots available for new tasks.")).toBeTruthy();
  expect(panel.querySelectorAll(".capacity-slots > span")).toHaveLength(16);
  expect(within(panel).getByText("Codex")).toBeTruthy();
  expect(within(panel).getByText("OpenCode")).toBeTruthy();
  await user.click(within(panel).getByRole("button", { name: "Adjust concurrency" }));
  expect(onPage).toHaveBeenCalledExactlyOnceWith("settings");

  const next = attentionSnapshot([]);
  next.capacity = {
    global: { active: 1, limit: 8 },
    by_trigger: {
      new_issue: { active: 0, limit: 4 },
      pr_review: { active: 1, limit: 3 },
      pr_approved: { active: 0, limit: 2 },
    },
  };
  view.rerender(<OverviewPage snapshot={next} {...props} />);
  expect(within(panel).getByLabelText("Global capacity").textContent).toBe("1 / 8");
  expect(within(panel).getByLabelText("New issue capacity").textContent).toBe("0 / 4");
  expect(within(panel).getByLabelText("PR review capacity").textContent).toBe("1 / 3");
  expect(within(panel).getByLabelText("PR approved capacity").textContent).toBe("0 / 2");
  expect(within(panel).getByText("7 slots available for new tasks.")).toBeTruthy();

  view.rerender(<OverviewPage snapshot={attentionSnapshot([])} {...props} />);
  expect(within(panel).getByLabelText("Global capacity").textContent).toBe("0 / 4");
  for (const label of ["New issue", "PR review", "PR approved"]) {
    expect(within(panel).getByLabelText(label + " capacity").textContent).toBe("0 / 4");
  }
  expect(within(panel).getByText("All execution slots are available.")).toBeTruthy();
});
