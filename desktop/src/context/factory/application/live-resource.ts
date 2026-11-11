/** Subscribe first, serialize snapshots, repeat if events arrived while loading. */
export class LiveResource<T> {
  private stopped = false;
  private loading = false;
  private dirty = false;
  private unlisten?: () => void;
  constructor(
    private adapter: {
      listen: (callback: () => void) => Promise<() => void>;
      load: () => Promise<T>;
    },
    private value: (data: T) => void,
    private error: (error: unknown) => void,
  ) {}
  async start() {
    try {
      const unlisten = await this.adapter.listen(() => void this.refresh());
      if (this.stopped) {
        unlisten();
        return;
      }
      this.unlisten = unlisten;
      await this.refresh();
    } catch (e) {
      if (!this.stopped) this.error(e);
    }
  }
  async refresh() {
    if (this.stopped) return;
    this.dirty = true;
    if (this.loading) return;
    this.loading = true;
    try {
      while (this.dirty && !this.stopped) {
        this.dirty = false;
        try {
          const result = await this.adapter.load();
          if (!this.stopped && !this.dirty) this.value(result);
        } catch (e) {
          if (!this.stopped && !this.dirty) this.error(e);
        }
      }
    } finally {
      this.loading = false;
    }
  }
  stop() {
    this.stopped = true;
    this.unlisten?.();
  }
}
