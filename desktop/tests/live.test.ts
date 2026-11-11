import { describe, it, expect } from "vitest";
import { LiveResource } from "../src/context/factory/application/live-resource";
const deferred = <T>() => {
  let resolve!: (x: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};
describe("live resource", () => {
  it("does not publish a rejected stale load after invalidation", async () => {
    let notify!: () => void;
    let reject!: (error: unknown) => void;
    let calls = 0;
    const values: number[] = [];
    const errors: unknown[] = [];
    const first = new Promise<number>((_, r) => { reject = r; });
    const resource = new LiveResource<number>({
      listen: async (cb) => { notify = cb; return () => {}; },
      load: () => ++calls === 1 ? first : Promise.resolve(2),
    }, value => values.push(value), error => errors.push(error));
    const started = resource.start();
    await Promise.resolve();
    notify();
    reject(new Error("obsolete detail"));
    await started;
    expect(errors).toEqual([]);
    expect(values).toEqual([2]);
    resource.stop();
  });
  it("subscribes before loading, refreshes after events during a snapshot, releases listener", async () => {
    const order: string[] = [];
    let event!: () => void;
    let stopped = false;
    let calls = 0;
    const first = deferred<number>();
    const values: number[] = [];
    const resource = new LiveResource<number>(
      {
        listen: async (cb) => {
          order.push("listen");
          event = cb;
          return () => {
            stopped = true;
          };
        },
        load: async () => {
          order.push("load");
          calls++;
          return calls === 1 ? first.promise : 2;
        },
      },
      (v) => values.push(v),
      () => {},
    );
    const start = resource.start();
    await Promise.resolve();
    event();
    first.resolve(1);
    await start;
    await new Promise((r) => setTimeout(r, 0));
    expect(order[0]).toBe("listen");
    expect(values.at(-1)).toBe(2);
    expect(calls).toBe(2);
    resource.stop();
    expect(stopped).toBe(true);
  });
  it("does not deliver old selection after stopping", async () => {
    const first = deferred<number>();
    const values: number[] = [];
    const resource = new LiveResource(
      { listen: async () => () => {}, load: () => first.promise },
      (v) => values.push(v),
      () => {},
    );
    const start = resource.start();
    await Promise.resolve();
    resource.stop();
    first.resolve(1);
    await start;
    expect(values).toEqual([]);
  });
});
