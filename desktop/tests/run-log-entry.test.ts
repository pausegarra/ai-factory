import { expect, it } from "vitest";
import { interpretLogLine } from "../src/context/factory/application/services/run-log-entry";
import {
  codexText,
  command,
  envelope,
  jsonLine,
  openText,
  tool,
} from "./fixtures/run-logs";

const parse = (value: unknown) => interpretLogLine(jsonLine(value), "line:0");

it.each([openText, codexText])("extracts text with Unicode and real newlines", (make) => {
  const entry = parse(make("Hola 🚀\nsegunda linea"));
  expect(entry.label).toBe("Agent message");
  expect(entry.blocks[0].text).toBe("Hola 🚀\nsegunda linea");
});

it("retains both common text fields and empty recognized messages", () => {
  expect(parse({ ...openText(""), item: { text: "second" } }).blocks.map((block) => block.text))
    .toEqual(["Empty message", "second"]);
  expect(parse({ type: "future", part: { text: "one" }, item: { text: "two" } }).label)
    .toBe("Agent output");
});

it.each(["item.started", "item.updated", "item.completed"])("projects command phase %s", (type) => {
  const entry = parse({ ...command, type });
  expect(entry.kind).toBe("tool");
  expect(entry.metadata).toContain(`Phase: ${type.slice(5)}`);
  expect(entry.blocks.map((block) => block.text)).toEqual(["npm run test", "Tests: 3 passed\n"]);
  expect(entry.metadata).toContain("Exit code: 0");
});

it("renders structured tool input and errors without shell assumptions", () => {
  expect(parse(tool).label).toBe("read");
  expect(parse(tool).blocks[0].text).toBe(JSON.stringify({ path: "README.md" }, null, 2));
  expect(parse({ ...command, item: { ...command.item, exit_code: 2 } }).metadata)
    .toContain("Command failed");
  expect(parse({
    type: "tool_use",
    part: { type: "tool", tool: "custom", state: { input: [1], output: { ok: true }, error: "denied" } },
  }).blocks.map((block) => block.text)).toEqual(["[\n  1\n]", "{\n  \"ok\": true\n}", "denied"]);
  expect(parse({ type: "item.started", item: { type: "command_execution" } }).blocks).toEqual([]);
});

it.each(["thread.started", "turn.started", "turn.completed", "step_start", "step_finish"])(
  "shows event %s",
  (type) => {
    const entry = parse({ type, thread_id: "thread-test", sessionID: "session-test" });
    expect(entry.kind).toBe("event");
    expect(entry.metadata).toContain("Thread: thread-test");
    expect(entry.metadata).toContain("Session: session-test");
  },
);

it.each([{ message: "bad" }, { error: "bad" }, { error: { message: "bad" } }])(
  "shows recognized errors",
  (fields) => {
    expect(parse({ type: "turn.failed", ...fields }).blocks[0].text).toBe("bad");
    expect(parse({ type: "error", ...fields }).label).toBe("Error");
  },
);

it.each([
  { type: "future_event", payload: { count: 2 } },
  [1, "two"],
  "JSON string",
  4,
  true,
  null,
  { type: "text", part: { text: 4 }, item: { text: "must not rescue" } },
  { type: "tool_use", part: { type: "tool", tool: 3, state: {}, text: "must not rescue" } },
  { type: "item.completed", item: { type: "command_execution", command: 3, text: "must not rescue" } },
])("falls back without losing incompatible record", (input) => {
  expect(parse(input).blocks[0].text).toBe(JSON.stringify(input, null, 2));
  expect(parse(input).label).toBe("Log entry");
});

it("keeps malformed and plain text literal and gives message-less errors a visible fallback", () => {
  for (const text of ["warning: local fixture unavailable", "error is ordinary text", '{"type":"text","part":']) {
    expect(interpretLogLine(text, "x").blocks[0].text).toBe(text);
    expect(interpretLogLine(text, "x").kind).toBe("plain");
  }
  expect(parse({ type: "error", extra: 1 }).label).toBe("Error");
  expect(parse({ type: "error", part: { text: "not error message" } }).blocks[0].text)
    .toContain('"type": "error"');
});

it("keeps source visible if excessively deep JSON cannot be pretty printed", () => {
  const source = "[".repeat(10000) + "0" + "]".repeat(10000);
  expect(interpretLogLine(source, "deep").blocks[0].text).toBe(source);
});

it("shows a valid envelope summary and remaining fields without executing actions", () => {
  const block = parse(openText(jsonLine(envelope))).blocks[0];
  expect(block.text).toBe(envelope.summary);
  expect(block.reportedStatus).toBe("succeeded");
  expect(JSON.parse(block.details!)).toEqual({ artifacts: [], data: { approved: true } });
  for (const dataJson of ['{"publish":{"title":"untrusted"}}', "not JSON"]) {
    const transport = { status: "blocked", summary: "Needs review", artifacts: [], data_json: dataJson };
    const details = parse(codexText(jsonLine(transport))).blocks[0].details!;
    expect(JSON.parse(details).data_json)
      .toEqual(dataJson === "not JSON" ? dataJson : JSON.parse(dataJson));
  }
  for (const ordinary of [
    { summary: "not envelope" },
    { ...envelope, status: "other" },
    { ...envelope, status: ["succeeded"] },
    { ...envelope, summary: " " },
    { ...envelope, data: [] },
  ]) {
    const text = jsonLine(ordinary);
    expect(parse(openText(text)).blocks[0].text).toBe(text);
    expect(parse(openText(text)).blocks[0].details).toBeUndefined();
  }
});
