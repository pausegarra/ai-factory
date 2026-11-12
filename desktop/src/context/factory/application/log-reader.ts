import type { LogChunk } from "../domain/types";

export interface LogReadSnapshot {
  text: string;
  eof: boolean;
}

/** IPC offsets count UTF-8 bytes, never JavaScript string characters. */
export class LogReader {
  private offset = 0;
  private current: LogReadSnapshot = { text: "", eof: false };
  private stopped = false;

  constructor(private load: (offset: number) => Promise<LogChunk>) {}

  snapshot(): LogReadSnapshot {
    return this.current;
  }

  async read(): Promise<string> {
    return (await this.readSnapshot()).text;
  }

  async readSnapshot(): Promise<LogReadSnapshot> {
    while (!this.stopped) {
      const chunk = await this.load(this.offset);
      if (this.stopped) return this.current;
      const advanced = chunk.next_offset > this.offset;
      if (chunk.text || chunk.eof !== this.current.eof) {
        this.current = { text: this.current.text + chunk.text, eof: chunk.eof };
      }
      this.offset = chunk.next_offset;
      if (chunk.eof || !advanced) return this.current;
    }
    return this.current;
  }

  stop(): void {
    this.stopped = true;
  }
}
