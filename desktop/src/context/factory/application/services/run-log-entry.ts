import type { LogBlock, LogEntry } from "../dto/run-log";

type Obj = Record<string, unknown>;

const object = (value: unknown): value is Obj =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const has = (value: Obj, key: string): boolean =>
  Object.prototype.hasOwnProperty.call(value, key);
const pretty = (value: unknown): string => JSON.stringify(value, null, 2) ?? String(value);
const display = (value: unknown): string => typeof value === "string" ? value : pretty(value);
const optional = (value: Obj, key: string, type: string): boolean =>
  !has(value, key) || typeof value[key] === type;

function message(text: string, id: string): LogBlock {
  const block: LogBlock = { id, text: text || "Empty message", format: "text" };
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return block;
  }
  if (!object(value) || typeof value.status !== "string" ||
      !["succeeded", "blocked", "failed"].includes(value.status) ||
      typeof value.summary !== "string" || !value.summary.trim() || !Array.isArray(value.artifacts)) {
    return block;
  }
  if (has(value, "data") ? !object(value.data) : typeof value.data_json !== "string") return block;

  const rest = { ...value };
  delete rest.status;
  delete rest.summary;
  if (!has(value, "data") && typeof rest.data_json === "string") {
    try {
      const decoded: unknown = JSON.parse(rest.data_json);
      if (object(decoded)) rest.data_json = decoded;
    } catch {
      // Keep original serialized field visible when it is not a JSON object.
    }
  }
  return { ...block, text: value.summary, reportedStatus: value.status, details: pretty(rest) };
}

export function interpretLogLine(line: string, id: string): LogEntry {
  try {
    return projectLogLine(line, id);
  } catch {
    return {
      id,
      kind: "plain",
      label: "",
      metadata: [],
      blocks: [{ id: `${id}:source`, text: line, format: "code" }],
    };
  }
}

function projectLogLine(line: string, id: string): LogEntry {
  const entry = (
    kind: LogEntry["kind"],
    label: string,
    blocks: readonly LogBlock[],
    metadata: readonly string[] = [],
  ): LogEntry => ({ id, kind, label, blocks, metadata });
  const code = (key: string, label: string, value: unknown): LogBlock => ({
    id: `${id}:${key}`,
    label,
    text: display(value),
    format: "code",
  });

  if (line === "") return entry("spacer", "", []);

  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return entry("plain", "", [code("source", "", line)]);
  }
  if (!object(parsed)) {
    return entry("fallback", "Log entry", [code("source", "", pretty(parsed))]);
  }

  const part = object(parsed.part) ? parsed.part : undefined;
  const item = object(parsed.item) ? parsed.item : undefined;
  const type = parsed.type;
  const fallback = (label = "Log entry"): LogEntry =>
    entry(label === "Error" ? "error" : "fallback", label, [code("source", "", pretty(parsed))]);

  if (typeof type === "string" && ["item.started", "item.updated", "item.completed"].includes(type) &&
      item?.type === "command_execution") {
    if (!optional(item, "command", "string") || !optional(item, "aggregated_output", "string") ||
        (has(item, "exit_code") &&
          (typeof item.exit_code !== "number" || !Number.isFinite(item.exit_code)))) return fallback();
    const blocks: LogBlock[] = [];
    if (typeof item.command === "string") blocks.push(code("command", "Command", item.command));
    if (typeof item.aggregated_output === "string") blocks.push(code("output", "Output", item.aggregated_output));
    const metadata = [`Phase: ${type.slice(5)}`];
    if (typeof item.exit_code === "number") {
      metadata.push(`Exit code: ${item.exit_code}`);
      if (item.exit_code !== 0) metadata.push("Command failed");
    }
    return entry("tool", "Command", blocks, metadata);
  }

  if (type === "tool_use") {
    if (!part || part.type !== "tool" || typeof part.tool !== "string" || !part.tool.trim() ||
        !object(part.state) || !optional(part.state, "status", "string")) return fallback();
    const state = part.state;
    const blocks: LogBlock[] = [];
    for (const [key, label] of [["input", "Input"], ["output", "Output"], ["error", "Error"]]) {
      if (has(state, key)) blocks.push(code(key, label, state[key]));
    }
    return entry("tool", part.tool, blocks, typeof state.status === "string" ? [`Status: ${state.status}`] : []);
  }

  if (type === "error" || type === "turn.failed") {
    const error = parsed.error;
    const text = [parsed.message, error, object(error) ? error.message : undefined]
      .find((value): value is string => typeof value === "string");
    return typeof text === "string"
      ? entry("error", "Error", [code("error", "", text || "Empty message")])
      : fallback("Error");
  }

  const events: Record<string, string> = {
    "thread.started": "Thread started",
    "turn.started": "Turn started",
    "turn.completed": "Turn completed",
    step_start: "Step started",
    step_finish: "Step finished",
  };
  if (typeof type === "string" && Object.prototype.hasOwnProperty.call(events, type)) {
    if (!optional(parsed, "thread_id", "string") || !optional(parsed, "sessionID", "string")) return fallback();
    const metadata: string[] = [];
    if (typeof parsed.thread_id === "string") metadata.push(`Thread: ${parsed.thread_id}`);
    if (typeof parsed.sessionID === "string") metadata.push(`Session: ${parsed.sessionID}`);
    return entry("event", events[type], [], metadata);
  }

  const recognized = type === "text" || (type === "item.completed" && item?.type === "agent_message");
  if ((type === "text" && (!part || typeof part.text !== "string")) ||
      (type === "item.completed" && item?.type === "agent_message" && typeof item.text !== "string")) {
    return fallback();
  }
  if ((part && has(part, "text") && typeof part.text !== "string") ||
      (item && has(item, "text") && typeof item.text !== "string")) return fallback();

  const blocks: LogBlock[] = [];
  if (part && typeof part.text === "string") blocks.push(message(part.text, `${id}:part.text`));
  if (item && typeof item.text === "string") blocks.push(message(item.text, `${id}:item.text`));
  return blocks.length
    ? entry("message", recognized ? "Agent message" : "Agent output", blocks)
    : fallback();
}
