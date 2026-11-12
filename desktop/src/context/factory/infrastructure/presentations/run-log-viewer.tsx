import { useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { LogBlock, LogDocument, LogViewMode } from "../../application/dto/run-log";
import { logPreview } from "../../application/services/log-preview";

function TextBlock({ block, expanded, onToggle }: {
  block: LogBlock;
  expanded: boolean;
  onToggle: () => void;
}) {
  const id = useId();
  const preview = useMemo(() => logPreview(block.text), [block.text]);
  return (
    <div className={`run-log-block run-log-${block.format}`}>
      {block.label && <span className="run-log-block-label">{block.label}</span>}
      <pre id={id}>{expanded ? block.text : preview.prefix}</pre>
      {preview.truncated && <>
        {!expanded && <span className="run-log-omitted">More content</span>}
        <button
          type="button"
          className="text-button"
          aria-expanded={expanded}
          aria-controls={id}
          aria-label={`${expanded ? "Show less" : "Show more"}${block.label ? ` ${block.label}` : " text"}`}
          onClick={onToggle}
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      </>}
    </div>
  );
}

function MessageBlock({ block, expanded, toggle }: {
  block: LogBlock;
  expanded: ReadonlySet<string>;
  toggle: (key: string) => void;
}) {
  const detailId = useId();
  const detailKey = `${block.id}:details-open`;
  const contentKey = `${block.id}:details-content`;
  return (
    <>
      <TextBlock block={block} expanded={expanded.has(block.id)} onToggle={() => toggle(block.id)} />
      {block.reportedStatus &&
        <span className="run-log-metadata">Reported status: {block.reportedStatus}</span>}
      {block.details !== undefined && <div className="run-log-result">
        <button
          type="button"
          className="text-button"
          aria-expanded={expanded.has(detailKey)}
          aria-controls={detailId}
          onClick={() => toggle(detailKey)}
        >
          Result details
        </button>
        <div id={detailId} hidden={!expanded.has(detailKey)}>
          {expanded.has(detailKey) && <TextBlock
            block={{ id: contentKey, text: block.details, format: "code" }}
            expanded={expanded.has(contentKey)}
            onToggle={() => toggle(contentKey)}
          />}
        </div>
      </div>}
    </>
  );
}

export function RunLogViewer({ document, mode, onModeChange, follow }: {
  document: LogDocument;
  mode: LogViewMode;
  onModeChange: (mode: LogViewMode) => void;
  follow: boolean;
}) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const viewport = useRef<HTMLDivElement>(null);
  const previousRaw = useRef("");
  const readingTop = useRef(0);
  const restoreTop = useRef<number | null>(null);

  function capture(): void {
    restoreTop.current = viewport.current?.scrollTop ?? 0;
  }

  function toggle(key: string): void {
    capture();
    setExpanded((old) => {
      const next = new Set(old);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  useLayoutEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const appended = document.raw !== previousRaw.current;
    if (appended && follow) element.scrollTop = element.scrollHeight;
    else if (restoreTop.current !== null || (appended && !follow)) {
      const top = restoreTop.current ?? readingTop.current;
      element.scrollTop = Math.max(0, Math.min(top, element.scrollHeight - element.clientHeight));
    }
    previousRaw.current = document.raw;
    readingTop.current = element.scrollTop;
    restoreTop.current = null;
  }, [document.raw, mode, follow, expanded]);

  return (
    <>
      <div className="run-log-toolbar" role="group" aria-label="Log view">
        {(["readable", "raw"] as const).map((value) => <button
          key={value}
          type="button"
          className={`button button-secondary ${mode === value ? "selected" : ""}`}
          aria-pressed={mode === value}
          onClick={() => {
            if (mode !== value) {
              capture();
              onModeChange(value);
            }
          }}
        >
          {value === "readable" ? "Readable" : "Raw"}
        </button>)}
      </div>
      <div
        className="log-output run-log-viewport"
        ref={viewport}
        role="region"
        aria-label="Session output"
        onScroll={() => { readingTop.current = viewport.current?.scrollTop ?? 0; }}
      >
        {!document.raw ? <p className="run-log-empty">Waiting for agent output…</p> :
          mode === "raw" ? <pre className="run-log-raw" data-testid="raw-log-output">{document.raw}</pre> :
            document.entries.map((entry) => entry.kind === "spacer"
              ? <div key={entry.id} className="run-log-spacer" aria-hidden="true">{"\n"}</div>
              : <div
                key={entry.id}
                className={`run-log-entry run-log-entry-${entry.kind}`}
                data-entry-id={entry.id}
              >
                {entry.label && <span className="run-log-label">{entry.label}</span>}
                {entry.metadata.map((text, index) =>
                  <span key={index} className="run-log-metadata">{text}</span>)}
                {entry.blocks.map((block) =>
                  <MessageBlock key={block.id} block={block} expanded={expanded} toggle={toggle} />)}
              </div>)}
      </div>
    </>
  );
}
