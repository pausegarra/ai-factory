import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { SourceConnector } from "../../domain/types";
import { parseSourceScope, validateSourceConnectors, type SourceScopeField } from "../../domain/source-connectors";
import { Field, Notice } from "./ui";

type SourceConnectorDialogProps = {
  id: string;
  connector: SourceConnector;
  mode: "new" | "existing";
  attempted: boolean;
  busy: boolean;
  onChange: (patch: Partial<SourceConnector>) => void;
  onApply: () => void;
  onCancel: () => void;
  onRemove?: () => void;
  returnFocus: () => HTMLElement | null;
};

function focusable(node: HTMLElement | null): HTMLElement[] {
  return Array.from(node?.querySelectorAll<HTMLElement>("button, input, select, textarea, a[href], [tabindex]") ?? [])
    .filter(control => !control.matches(":disabled, [hidden], [type=hidden]") && control.tabIndex >= 0 && !control.closest("[hidden], [inert]"));
}

export function SourceConnectorDialog({id, connector: c, mode, attempted, busy, onChange, onApply, onCancel, onRemove, returnFocus}: SourceConnectorDialogProps) {
  const dialog = useRef<HTMLElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const restore = useRef(returnFocus);
  restore.current = returnFocus;
  useLayoutEffect(() => {
    const node = dialog.current;
    const invalid = node?.querySelector<HTMLElement>('[aria-invalid="true"]');
    const target = invalid && (focusable(node).includes(invalid) ? invalid : focusable(invalid)[0]);
    (target || close.current)?.focus();
    const contain = (event: FocusEvent) => {
      if (node?.isConnected && event.target instanceof Node && !node.contains(event.target)) close.current?.focus();
    };
    document.addEventListener("focusin", contain);
    return () => {
      document.removeEventListener("focusin", contain);
      queueMicrotask(() => {
        if (node?.isConnected) return;
        // A replacement dialog may already own focus (for example, validation
        // immediately after Apply). Its initial error focus must take precedence.
        if (document.activeElement?.closest('[role="dialog"], [role="alertdialog"]')) return;
        const opener = restore.current();
        const target = opener?.isConnected && !opener.matches(":disabled") && !opener.closest("[inert]")
          ? opener : document.querySelector<HTMLElement>("main:not([inert])");
        target?.focus();
      });
    };
  }, []);
  const errors = attempted ? validateSourceConnectors([c]) : [];
  return createPortal(<div className="dialog-backdrop notification-connector-backdrop" onKeyDown={event => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!busy) onCancel(); return; }
    if (event.key !== "Tab") return;
    const controls = focusable(dialog.current), first = controls[0], last = controls.at(-1);
    if (!first || !last) { event.preventDefault(); return; }
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  }}>
    <section ref={dialog} id={id} role="dialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
      className="confirm-dialog notification-connector-dialog">
      <div className="connector-picker-heading">
        <h2 id={`${id}-title`}>{mode === "new" ? "Configure new connector" : "Edit connector"}</h2>
        <button ref={close} type="button" className="button button-secondary" disabled={busy} aria-label="Close connector editor" onClick={onCancel}>Close</button>
      </div>
      <p id={`${id}-description`}>Changes apply to this page. Use Save changes to save your settings.</p>
      {errors.length > 0 && <Notice>{errors.join(" ")}</Notice>}
      <fieldset className="notifications-connector-content" disabled={busy}>
        <div className="form-grid">
          <Field label="Connector type"><select defaultValue="github_issues" disabled><option value="github_issues">GitHub Issues</option></select></Field>
          <Field label="Name"><input value={c.label} aria-invalid={attempted && !c.label.trim()} onChange={e => onChange({label:e.target.value})} /></Field>
          <div className="source-account-fields">
            <Field label="GitHub account"><input value={c.account} autoCapitalize="none" spellCheck={false} aria-invalid={attempted && !c.account.trim()} onChange={e => onChange({account:e.target.value, has_token:false})} /></Field>
            <label className="checkbox-label source-review-scope">
              <input type="checkbox" checked={c.review_all_open_pull_requests ?? false}
                aria-describedby={`${id}-review-scope-hint`}
                onChange={e => onChange({review_all_open_pull_requests:e.target.checked})} />
              <span>Review all open PRs in configured owners</span>
            </label>
            <small className="footnote" id={`${id}-review-scope-hint`}>
              When off, only PRs with a review requested from this account are discovered. Applies only to PR review.
            </small>
            {c.review_all_open_pull_requests && <SourceScopeInput
              field="pull_request_authors" label="Allowed PR authors" source={c} onChange={onChange}
              hint="Separate GitHub logins with commas. Empty includes all authors in configured owners. Applies only when reviewing all open PRs."
            />}
          </div>
          <Field label="API token" hint="Leave blank to keep a configured token."><input type="password" autoComplete="new-password" spellCheck={false} value={c.token ?? ""} aria-invalid={attempted && !c.has_token && !c.token?.trim()} onChange={e => onChange({token:e.target.value})} /></Field>
        </div>
        <p>{c.has_token ? "Token configured" : "Token not configured"}</p>
        <div className="form-grid">
          {([ ["owners", "GitHub owners"], ["authors", "Allowed authors"], ["reviewers", "Allowed reviewers"] ] as const).map(([field,label]) => <SourceScopeInput key={field} field={field} label={label} source={c} onChange={onChange} />)}
        </div>
      </fieldset>
      <div className="notifications-connector-actions" role="group" aria-label="Connector actions">
        {onRemove && <button type="button" className="button button-secondary" disabled={busy} onClick={onRemove} aria-label={`Remove connector ${c.label || "Connector"}`}>Remove connector</button>}
        <button type="button" className="button button-secondary" disabled={busy} onClick={onCancel}>Cancel</button>
        <button type="button" className="button button-primary" disabled={busy} onClick={onApply}>Apply changes</button>
      </div>
    </section>
  </div>, document.body);
}

function SourceScopeInput({field, label, source, onChange, hint = "Separate GitHub logins with commas."}: {
  field:SourceScopeField; label:string; source:SourceConnector;
  onChange:(patch:Partial<SourceConnector>)=>void; hint?:string;
}) {
  const values = source[field] ?? [];
  const [text, setText] = useState(() => values.join(", "));
  useEffect(() => {
    if (JSON.stringify(parseSourceScope(text)) !== JSON.stringify(values)) setText(values.join(", "));
  }, [source[field]]);
  return <Field label={label} hint={hint}><input value={text} autoCapitalize="none" spellCheck={false} onChange={e => {setText(e.target.value); onChange({[field]:parseSourceScope(e.target.value)});}} /></Field>;
}
