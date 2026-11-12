import { it, expect } from "vitest";
import { LogReader } from "../src/context/factory/application/log-reader";
it("stops draining when an in-flight chunk resolves after stop", async () => {
  let resolve!: (chunk: { text: string; offset: number; next_offset: number; eof: boolean }) => void;
  let calls = 0;
  const reader = new LogReader(() => {
    calls++;
    return new Promise(r => { resolve = r; });
  });
  const reading = reader.read();
  reader.stop();
  resolve({ text: "obsolete", offset: 0, next_offset: 8, eof: false });
  expect(await reading).toBe("");
  expect(calls).toBe(1);
  expect(await reader.read()).toBe("");
  expect(calls).toBe(1);
});
it("drains all available chunks without needing another event", async () => {
  const chunks = [
    { text: "first", offset: 0, next_offset: 5, eof: false },
    { text: "second", offset: 5, next_offset: 11, eof: true },
  ];
  const offsets: number[] = [];
  const reader = new LogReader(async (offset) => {
    offsets.push(offset);
    return chunks.shift()!;
  });
  expect(await reader.read()).toBe("firstsecond");
  expect(offsets).toEqual([0, 5]);
});
it("stops at current end of a running session and resumes from byte offset", async () => {
  let calls = 0;
  const reader = new LogReader(async (offset) => {
    calls++;
    return calls === 1
      ? { text: "live", offset, next_offset: 4, eof: false }
      : calls === 2
        ? { text: "", offset, next_offset: 4, eof: false }
        : { text: "end", offset, next_offset: 7, eof: true };
  });
  expect(await reader.read()).toBe("live");
  expect(calls).toBe(2);
  expect(await reader.read()).toBe("liveend");
});

it("reports definitive eof without new text and keeps snapshot identity on no-op", async () => {
  const chunks = [
    { text: '{"type":"text"}', offset: 0, next_offset: 90, eof: false },
    { text: "", offset: 90, next_offset: 90, eof: false },
    { text: "", offset: 90, next_offset: 90, eof: false },
    { text: "", offset: 90, next_offset: 90, eof: true },
  ];
  const reader = new LogReader(async () => chunks.shift()!);
  const first = await reader.readSnapshot();
  expect(first.eof).toBe(false);
  expect(await reader.readSnapshot()).toBe(first);
  expect(await reader.readSnapshot()).toEqual({ text: first.text, eof: true });
});

it("uses supplied offsets through Unicode, redaction and partial read failure", async () => {
  const offsets: number[] = [];
  let calls = 0;
  const reader = new LogReader(async (offset) => {
    offsets.push(offset);
    if (++calls === 2) throw new Error("IPC offline");
    return calls === 1
      ? { text: "á🚀[REDACTED]", offset, next_offset: 80, eof: false }
      : { text: " done", offset, next_offset: 95, eof: true };
  });
  await expect(reader.readSnapshot()).rejects.toThrow("IPC offline");
  expect(reader.snapshot()).toEqual({ text: "á🚀[REDACTED]", eof: false });
  expect(await reader.read()).toBe("á🚀[REDACTED] done");
  expect(offsets).toEqual([0, 80, 80]);
});

it("does not apply eof or content after stopping a pending snapshot read", async () => {
  let resolve!: (chunk: { text: string; offset: number; next_offset: number; eof: boolean }) => void;
  let calls = 0;
  const reader = new LogReader(() => {
    calls++;
    return new Promise((done) => { resolve = done; });
  });
  const reading = reader.readSnapshot();
  reader.stop();
  resolve({ text: "obsolete", offset: 0, next_offset: 800, eof: true });
  expect(await reading).toEqual({ text: "", eof: false });
  expect(await reader.readSnapshot()).toBe(reader.snapshot());
  expect(calls).toBe(1);
});
