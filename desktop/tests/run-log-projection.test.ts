import { expect, it } from "vitest";
import { RunLogProjection } from "../src/context/factory/application/services/run-log-projection";
import { command, jsonLine, openText } from "./fixtures/run-logs";

it("keeps provisional tail in the same slot and finalizes at eof without append", () => {
  const projection = new RunLogProjection();
  const raw = jsonLine(openText("á🚀\nnext"));
  const first = projection.update({ text: raw.slice(0, 9), eof: false });
  expect(first.entries[0].kind).toBe("provisional");
  const completeButOpen = projection.update({ text: raw, eof: false });
  expect(completeButOpen.entries[0].kind).toBe("provisional");
  const done = projection.update({ text: raw, eof: true });
  expect(done.entries).toHaveLength(1);
  expect(done.entries[0].id).toBe(first.entries[0].id);
  expect(done.entries[0].blocks[0].text).toBe("á🚀\nnext");
  expect(projection.update({ text: raw, eof: true })).toBe(done);
});

it("is equivalent at every code point split, including escapes, CRLF and many chunks", () => {
  const raw = jsonLine(openText("á🚀\nnext")) + "\r\n\r\n" + jsonLine(command) + '\n{"bad":';
  const expected = new RunLogProjection().update({ text: raw, eof: true });
  const points = Array.from(raw);
  for (let cut = 0; cut <= points.length; cut++) {
    const projection = new RunLogProjection();
    const prefix = points.slice(0, cut).join("");
    projection.update({ text: prefix, eof: false });
    projection.update({ text: prefix, eof: false });
    expect(projection.update({ text: raw, eof: true })).toEqual(expected);
  }
  const incremental = new RunLogProjection();
  let text = "";
  for (const point of points) {
    text += point;
    incremental.update({ text, eof: false });
  }
  expect(incremental.update({ text, eof: true })).toEqual(expected);
  expect(expected.raw).toBe(raw);
  expect(expected.entries.at(-1)?.kind).toBe("plain");
});

it("preserves complete entry references, repeated lines and whitespace", () => {
  const raw = "same\nsame\n\n";
  const projection = new RunLogProjection();
  const before = projection.update({ text: raw, eof: false });
  const after = projection.update({ text: raw + "end\n", eof: false });
  expect(after.entries.slice(0, 3)).toEqual(before.entries);
  expect(after.entries[0]).toBe(before.entries[0]);
  expect(new Set(after.entries.map((entry) => entry.id)).size).toBe(4);
  expect(after.entries[2].kind).toBe("spacer");

  const afterEof = new RunLogProjection();
  afterEof.update({ text: "one", eof: true });
  expect(afterEof.update({ text: "one two\n", eof: true }).entries[0].blocks[0].text).toBe("one two");
});

it("does not lose 1000 records or a 256 KiB block", () => {
  const large = "z".repeat(256 * 1024);
  const raw = Array.from({ length: 1000 }, (_, index) => jsonLine(openText(`record ${index}`))).join("\n") +
    "\n" + jsonLine(openText(large));
  const document = new RunLogProjection().update({ text: raw, eof: true });
  expect(document.entries).toHaveLength(1001);
  expect(document.entries[999].blocks[0].text).toBe("record 999");
  expect(document.entries[1000].blocks[0].text).toBe(large);
  expect(document.raw).toBe(raw);
});
