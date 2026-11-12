import type { LogChunk } from "../domain/types";
/** IPC offsets count UTF-8 bytes, never JavaScript string characters. */
export class LogReader {
  private offset = 0;
  private text = "";
  private stopped = false;
  constructor(private load: (offset: number) => Promise<LogChunk>) {}
  async read() {
    while (!this.stopped) {
      const chunk = await this.load(this.offset);
      if (this.stopped) return this.text;
      const advanced = chunk.next_offset > this.offset;
      this.text += chunk.text;
      this.offset = chunk.next_offset;
      if (chunk.eof || !advanced) return this.text;
    }
    return this.text;
  }
  stop() {
    this.stopped = true;
  }
}
