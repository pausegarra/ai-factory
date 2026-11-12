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
