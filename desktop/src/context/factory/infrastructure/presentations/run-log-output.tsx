import { useEffect, useRef, useState } from "react";
import type { LogDocument, LogViewMode } from "../../application/dto/run-log";
import { LogReader } from "../../application/log-reader";
import { LiveResource } from "../../application/live-resource";
import { RunLogProjection } from "../../application/services/run-log-projection";
import { factoryApi } from "../ipc/factory-api";
import { Notice } from "./ui";
import { noticeText } from "./notice-content";
import { RunLogViewer } from "./run-log-viewer";

const EMPTY_DOCUMENT: LogDocument = { raw: "", eof: false, entries: [] };
const DELETION_EVENTS = ["run_deleted", "run_deletion_pending", "run_deletion_failed"];

export function RunLogOutput({ runId, nodeId, mode, onModeChange, follow, onReadError }: {
  runId: string;
  nodeId: string;
  mode: LogViewMode;
  onModeChange: (mode: LogViewMode) => void;
  follow: boolean;
  onReadError: () => void;
}) {
  const [document, setDocument] = useState<LogDocument>(EMPTY_DOCUMENT);
  const [error, setError] = useState("");
  const readError = useRef(onReadError);
  readError.current = onReadError;

  useEffect(() => {
    if (!nodeId) return;
    const reader = new LogReader((offset) => factoryApi.logs(runId, nodeId, offset));
    const projection = new RunLogProjection();
    const resource = new LiveResource(
      {
        listen: (callback) => factoryApi.subscribe((event) => {
          if (event.kind === "resync") {
            callback();
          } else if (event.run_id === runId && !DELETION_EVENTS.includes(event.kind) &&
              (!event.node_id || event.node_id === nodeId)) {
            callback();
          }
        }),
        load: () => reader.readSnapshot(),
      },
      (snapshot) => {
        setDocument(projection.update(snapshot));
        setError("");
      },
      (reason) => {
        setDocument(projection.update(reader.snapshot()));
        setError(noticeText("Could not read run logs.", reason));
        readError.current();
      },
    );
    void resource.start();
    return () => {
      reader.stop();
      resource.stop();
    };
  }, [runId, nodeId]);

  return (
    <>
      {error && <Notice>{error}</Notice>}
      <RunLogViewer document={document} mode={mode} onModeChange={onModeChange} follow={follow} />
    </>
  );
}
