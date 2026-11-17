import { useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import type { NotificationConnector, SourceConnector } from "../../domain/types";
import { isTelegramConnector, isGoogleChatConnector, validateNotificationConnectors } from "../../domain/notification-connectors";
import { Field, Notice, Toggle } from "./ui";
import { NotificationConnectorFilters } from "./notification-connector-filters";
import { GoogleChatConnectorEditor } from "./google-chat-connector";

type NotificationConnectorDialogProps = {
  id: string;
  connector: NotificationConnector;
  sources: SourceConnector[];
  mode: "new" | "existing";
  attempted: boolean;
  busy: boolean;
  onChange: (patch: Partial<NotificationConnector>) => void;
  onApply: () => void;
  onCancel: () => void;
  onRemove?: () => void;
  returnFocus: () => HTMLElement | null;
};

function focusable(node: HTMLElement | null): HTMLElement[] {
  return Array.from(node?.querySelectorAll<HTMLElement>("button, input, select, textarea, a[href], [tabindex]") ?? [])
    .filter(control => !control.matches(":disabled, [hidden], [type=hidden]") && control.tabIndex >= 0 && !control.closest("[hidden], [inert]"));
}

export function NotificationConnectorDialog({id, connector: c, sources, mode, attempted, busy, onChange, onApply, onCancel, onRemove, returnFocus}: NotificationConnectorDialogProps) {
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
  const supported = isTelegramConnector(c) || isGoogleChatConnector(c);
  const errors = attempted && supported ? validateNotificationConnectors([c], sources) : [];
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
        {isTelegramConnector(c) ? <>
          <div className="form-grid">
            <Field label="Connector type"><select defaultValue="telegram" disabled><option value="telegram">Telegram</option></select></Field>
            <Field label="Name" hint={attempted && !c.label.trim() ? "Name is required." : undefined}>
              <input value={c.label} aria-invalid={attempted && !c.label.trim()} onChange={e => onChange({label: e.target.value})} />
            </Field>
          </div>
          <Toggle label="Enabled" checked={c.enabled} onChange={enabled => onChange({enabled})} />
          <div className="form-grid">
            <Field label="Chat ID" hint={attempted && c.enabled && !c.config.chat_id.trim() ? "Chat ID is required." : "Destination chat or channel ID."}>
              <input value={c.config.chat_id} aria-invalid={attempted && c.enabled && !c.config.chat_id.trim()} onChange={e => onChange({config: {...c.config, chat_id: e.target.value}})} />
            </Field>
            <Field label="Bot token" hint={attempted && c.enabled && !c.config.has_token && !c.config.bot_token?.trim() ? "Bot token is required." : "Leave blank to keep a configured token."}>
              <input type="password" autoComplete="new-password" spellCheck={false} value={c.config.bot_token ?? ""}
                aria-invalid={attempted && c.enabled && !c.config.has_token && !c.config.bot_token?.trim()}
                onChange={e => onChange({config: {...c.config, bot_token: e.target.value}})} />
            </Field>
          </div>
          <p>{c.config.has_token ? "Token configured" : "Token not configured"}</p>
          <NotificationConnectorFilters
            id={c.id} enabled={c.enabled} events={c.events}
            triggers={c.config.triggers} tokenProfileIds={c.config.token_profile_ids}
            sources={sources} attempted={attempted}
            onChange={({events, triggers, token_profile_ids}) => onChange({events, config: {...c.config, triggers, token_profile_ids}})}
          />
        </> : isGoogleChatConnector(c) ? <GoogleChatConnectorEditor connector={c} sources={sources} busy={busy} attempted={attempted} edit={onChange} /> : <p>Unsupported connector type: {c.type}</p>}
      </fieldset>
      <div className="notifications-connector-actions" role="group" aria-label="Connector actions">
        {onRemove && <button type="button" className="button button-secondary" disabled={busy} onClick={onRemove} aria-label={`Remove connector ${c.label || "Connector"}`}>Remove connector</button>}
        <button type="button" className="button button-secondary" disabled={busy} onClick={onCancel}>Cancel</button>
        {supported && <button type="button" className="button button-primary" disabled={busy} onClick={onApply}>Apply changes</button>}
      </div>
    </section>
  </div>, document.body);
}
