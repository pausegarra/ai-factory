import type { LogReadSnapshot } from "../log-reader";
import type { LogDocument, LogEntry } from "../dto/run-log";
import { interpretLogLine } from "./run-log-entry";

export class RunLogProjection {
  private complete: LogEntry[] = [];
  private lineStart = 0;
  private scanned = 0;
  private current: LogDocument = { raw: "", eof: false, entries: [] };

  update(input: LogReadSnapshot): LogDocument {
    if (input.text === this.current.raw && input.eof === this.current.eof) return this.current;

    // Input is an accumulated append-only snapshot from one LogReader instance.
    let lf = input.text.indexOf("\n", this.scanned);
    while (lf >= 0) {
      const end = lf > this.lineStart && input.text[lf - 1] === "\r" ? lf - 1 : lf;
      this.complete.push(interpretLogLine(input.text.slice(this.lineStart, end), `line:${this.lineStart}`));
      this.lineStart = lf + 1;
      lf = input.text.indexOf("\n", this.lineStart);
    }

    this.scanned = input.text.length;
    const entries = this.complete.slice();
    const tail = input.text.slice(this.lineStart);
    if (tail) {
      const id = `line:${this.lineStart}`;
      entries.push(input.eof
        ? interpretLogLine(tail, id)
        : {
            id,
            kind: "provisional",
            label: "Receiving output",
            metadata: [],
            blocks: [{ id: `${id}:source`, text: tail, format: "code" }],
          });
    }

    this.current = { raw: input.text, eof: input.eof, entries };
    return this.current;
  }
}
