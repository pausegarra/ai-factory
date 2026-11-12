export type LogViewMode = "readable" | "raw";

export interface LogBlock {
  id: string;
  label?: string;
  text: string;
  format: "text" | "code";
  reportedStatus?: string;
  details?: string;
}

export interface LogEntry {
  id: string;
  kind: "message" | "tool" | "event" | "error" | "plain" | "fallback" | "provisional" | "spacer";
  label: string;
  metadata: readonly string[];
  blocks: readonly LogBlock[];
}

export interface LogDocument {
  raw: string;
  eof: boolean;
  entries: readonly LogEntry[];
}
