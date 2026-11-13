import { expect, it } from "vitest";
import {
  importSummary,
  noticeDetail,
  noticeText,
} from "../../../../../src/context/factory/infrastructure/presentations/notice-content";

it.each([undefined, null, {}, "", "  ", "undefined", "null", "[object Object]"])(
  "omits unusable detail %s",
  (value) => {
    expect(noticeText("Could not save settings.", value)).toBe(
      "Could not save settings.",
    );
  },
);

it("normalizes Error messages without rewriting opaque text", () => {
  expect(noticeDetail(new Error(" denied "))).toBe("denied");
  expect(noticeDetail(" denied ")).toBe("denied");
  expect(noticeDetail("Error: denied")).toBe("Error: denied");
  expect(noticeText("Could not save settings.", new Error("path\n/long/file"))).toBe(
    "Could not save settings.\npath\n/long/file",
  );
});

it("does not duplicate a detail equal to its title", () => {
  expect(noticeText("Could not save settings.", "Could not save settings.")).toBe(
    "Could not save settings.",
  );
});

it("preserves long and HTML-like diagnostics literally", () => {
  const long = "x".repeat(4096);
  const html = '<script>window.noticeInjected = true</script>';
  expect(noticeText("Could not save settings.", long)).toBe(
    `Could not save settings.\n${long}`,
  );
  expect(noticeText("Could not save settings.", html)).toBe(
    `Could not save settings.\n${html}`,
  );
});

it.each([
  [0, "0 runs imported."],
  [1, "1 run imported."],
  [2, "2 runs imported."],
] as const)("reports %i imported runs", (count, message) => {
  expect(importSummary(count, false)).toBe(message);
  expect(importSummary(count, true)).toBe(`${message} Settings imported.`);
});
