import { expect, it } from "vitest";
import { logPreview } from "../src/context/factory/application/services/log-preview";

it.each([
  "",
  "x".repeat(1200),
  "🚀".repeat(1200),
  Array(12).fill("x").join("\n"),
  Array(12).fill("x").join("\r\n") + "\r\n",
])("does not fold exact boundary", (text) => {
  expect(logPreview(text)).toEqual({ prefix: text, truncated: false });
});

it.each(["\n", "\r\n"])("cuts at end of logical line 12 with %j", (separator) => {
  const prefix = Array(12).fill("line").join(separator);
  expect(logPreview(prefix + separator + "13")).toEqual({ prefix, truncated: true });
});

it("counts code points including separators and selects the first limit", () => {
  expect(logPreview("🚀".repeat(1201))).toEqual({ prefix: "🚀".repeat(1200), truncated: true });
  expect(logPreview("x".repeat(1201))).toEqual({ prefix: "x".repeat(1200), truncated: true });
  expect(logPreview("x".repeat(1199) + "\r\nrest").prefix).toBe("x".repeat(1199));
  expect(logPreview("x".repeat(1198) + "\r\nrest").prefix).toBe("x".repeat(1198) + "\r\n");
  expect(logPreview("x".repeat(1199) + "\nrest").prefix).toBe("x".repeat(1199) + "\n");
  expect(logPreview("x".repeat(1199) + "🚀rest").prefix).toBe("x".repeat(1199) + "🚀");
  expect(logPreview(Array(13).fill("z".repeat(101)).join("\n")).prefix).toHaveLength(1200);
  expect(logPreview(Array(13).fill("z").join("\n")).prefix).toBe(Array(12).fill("z").join("\n"));
});
