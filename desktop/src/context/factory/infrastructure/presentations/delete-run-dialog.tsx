import { useLayoutEffect, useRef } from "react";
import type { Run } from "../../domain/types";

export function DeleteRunDialog({ run, busy, onCancel, onConfirm }: {
  run: Pick<Run, "id" | "title" | "repository" | "number" | "attempt">;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancel = useRef<HTMLButtonElement>(null);
  useLayoutEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    cancel.current?.focus();
    return () => { if (opener?.isConnected && !(opener instanceof HTMLButtonElement && opener.disabled)) opener.focus(); };
  }, []);
  return <div className="dialog-backdrop" onKeyDown={event => {
    if (event.key === "Escape" && !busy) { event.preventDefault(); onCancel(); }
    if (event.key === "Tab") {
      const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const first = buttons[0], last = buttons.at(-1);
      if (!first || !last) { event.preventDefault(); return; }
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    }
  }}>
    <section role="alertdialog" aria-modal="true" aria-labelledby="delete-run-heading"
      aria-describedby="delete-run-description" className="confirm-dialog">
      <h2 id="delete-run-heading">Delete run?</h2>
      <p>{run.title}</p>
      <p>{run.repository} #{run.number} · Attempt {run.attempt}</p>
      <code>{run.id}</code>
      <p id="delete-run-description">This permanently deletes this run and its local logs, results, artifacts, memory and remaining workspace. This cannot be undone. GitHub resources, other runs, shared import archives and minimal automation safety records are kept.</p>
      <div>
        <button ref={cancel} className="button button-secondary" disabled={busy} onClick={onCancel}>Cancel</button>
        <button className="button button-danger" disabled={busy} onClick={onConfirm}>{busy ? "Deleting..." : "Delete run"}</button>
      </div>
    </section>
  </div>;
}
