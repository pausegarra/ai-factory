import { useEffect, useRef, useState } from "react";
import { Bell } from "@phosphor-icons/react";
import type { NotificationConnector, Settings } from "../../domain/types";
import { cleanNotificationConnectors, isTelegramConnector, isGoogleChatConnector, newGoogleChatConnector, newTelegramConnector, notificationEvents, validateNotificationConnectors } from "../../domain/notification-connectors";
import { Empty, Field, Notice, Toggle } from "./ui";
import { ConnectorPicker, type ConnectorKind } from "./connector-picker";
import { GoogleChatConnectorEditor } from "./google-chat-connector";

export function NotificationsPage({ settings, onSave, onDirtyChange, onBusyChange }: {
  settings: Settings;
  onSave: (settings: Settings) => Promise<Settings | void>;
  onDirtyChange?: (dirty: boolean) => void;
  onBusyChange?: (busy: boolean) => void;
}) {
  const publicConnectors = () => cleanNotificationConnectors(settings.notification_connectors ?? []);
  const [baseline, setBaseline] = useState(publicConnectors);
  const [draft, setDraft] = useState(publicConnectors);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const serial = useRef(0);
  const lastProps = useRef(JSON.stringify(publicConnectors()));
  const addButton = useRef<HTMLButtonElement>(null);
  const editors = useRef(new Map<string, HTMLFieldSetElement>());
  const dirty = JSON.stringify(draft) !== JSON.stringify(baseline);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; serial.current += 1; };
  }, []);
  useEffect(() => {
    const incoming = publicConnectors();
    const key = JSON.stringify(incoming);
    if (lastProps.current === key) return;
    lastProps.current = key;
    if (inFlight.current) return;
    if (!dirty) { setBaseline(incoming); setDraft(incoming); }
    else {
      // Credential presence is public metadata, independent of the editable draft.
      const refreshTokenHints = (items: NotificationConnector[]) => items.map(c => {
        if (isGoogleChatConnector(c)) {
          const persisted = incoming.find(saved => saved.id === c.id && isGoogleChatConnector(saved));
          const has_webhook_url = !!persisted && isGoogleChatConnector(persisted) && persisted.config.has_webhook_url;
          return {...c, config: {...c.config, has_webhook_url}};
        }
        if (!isTelegramConnector(c)) return c;
        const persisted = incoming.find(saved => saved.id === c.id && isTelegramConnector(saved));
        const has_token = !!persisted && isTelegramConnector(persisted) && persisted.config.has_token;
        return {...c, config: {...c.config, has_token}};
      });
      setBaseline(refreshTokenHints);
      setDraft(refreshTokenHints);
    }
  }, [settings]);
  useEffect(() => { onDirtyChange?.(dirty); return () => onDirtyChange?.(false); }, [dirty, onDirtyChange]);
  useEffect(() => { onBusyChange?.(busy); return () => onBusyChange?.(false); }, [busy, onBusyChange]);
  function edit(id: string, patch: Partial<NotificationConnector>) {
    setDraft(items => items.map(c => c.id === id ? {...c, ...patch} : c));
    setError(""); setSaved(false);
  }
  function add(kind: ConnectorKind) {
    const id = crypto.randomUUID();
    setDraft(items => [...items, kind === "google_chat" ? newGoogleChatConnector(id) : newTelegramConnector(id)]);
    setSaved(false); setError("");
    queueMicrotask(() => { if (mounted.current) editors.current.get(id)?.focus(); });
  }
  function remove(id: string) {
    const remaining = draft.filter(c => c.id !== id);
    const index = draft.findIndex(c => c.id === id);
    setDraft(remaining); setSaved(false); setError("");
    const next = remaining[Math.min(index, remaining.length - 1)]?.id;
    queueMicrotask(() => { if (mounted.current) (next ? editors.current.get(next) : addButton.current)?.focus(); });
  }
  function discard() {
    const clean = publicConnectors(); setBaseline(clean); setDraft(clean);
    setError(""); setSaved(false); setAttempted(false);
  }
  async function save() {
    if (inFlight.current) return;
    const connectors = structuredClone(draft).map(c => {
      if (isGoogleChatConnector(c)) {
        const existing = baseline.find(saved => saved.id === c.id && isGoogleChatConnector(saved));
        c.config.has_webhook_url = !!existing && isGoogleChatConnector(existing) && existing.config.has_webhook_url;
        const url = c.config.webhook_url?.trim();
        if (url) c.config.webhook_url = url; else delete c.config.webhook_url;
      }
      if (isTelegramConnector(c)) {
        const existing = baseline.find(saved => saved.id === c.id && isTelegramConnector(saved));
        c.config.has_token = !!existing && isTelegramConnector(existing) && existing.config.has_token;
        c.config.chat_id = c.config.chat_id.trim();
        const token = c.config.bot_token?.trim();
        if (token) c.config.bot_token = token; else delete c.config.bot_token;
      }
      return c;
    });
    setAttempted(true); setSaved(false);
    const errors = validateNotificationConnectors(connectors, settings.token_profiles);
    if (errors.length) { setError(errors.join(" ")); return; }
    const ticket = ++serial.current;
    inFlight.current = true; setBusy(true); setError("");
    const current = () => mounted.current && ticket === serial.current;
    try {
      const next = {...settings, notification_connectors: connectors};
      const result = await onSave(next);
      const stored = result?.notification_connectors ?? connectors.map(c => isTelegramConnector(c)
        ? {...c, config: {...c.config, has_token: c.config.has_token || !!c.config.bot_token}}
        : isGoogleChatConnector(c) ? {...c, config: {...c.config, has_webhook_url: c.config.has_webhook_url || !!c.config.webhook_url}} : c);
      const clean = cleanNotificationConnectors(stored.map(c => isGoogleChatConnector(c) ? {...c, config: {...c.config, has_webhook_url: c.config.has_webhook_url || !!c.config.webhook_url}} : c));
      if (current()) { setDraft(clean); setBaseline(clean); setSaved(true); setAttempted(false); }
    } catch {
      if (current()) setError("Could not save notification settings. Check your configuration and try again.");
    } finally {
      if (current()) { inFlight.current = false; setBusy(false); }
    }
  }
  return <section className="notifications-page" aria-labelledby="notifications-title">
    <div className="page-title">
      <div><h1 id="notifications-title">Notifications</h1><p>Choose which run updates each connector sends.</p></div>
      <ConnectorPicker disabled={busy} buttonRef={addButton} onSelect={add} />
    </div>
    <div className="notifications-content">
      {error && <Notice>{error}</Notice>}
      {saved && <Notice tone="success">Notification settings saved.</Notice>}
      {!draft.length ? <Empty icon={<Bell size={28} />} title="No notification connectors" description="Add a connector to receive run updates outside AI Factory." /> :
        <div className="settings-stack">
          {draft.map(c => <fieldset key={c.id} className="panel settings-section notifications-connector" disabled={busy} tabIndex={-1}
            ref={element => { if (element) editors.current.set(c.id, element); else editors.current.delete(c.id); }}>
            <legend>{c.label || "Connector"}</legend>
            <div className="notifications-connector-content">
              {isTelegramConnector(c) ? <>
                <div className="form-grid">
                  <Field label="Connector type"><select defaultValue="telegram" disabled={busy || baseline.some(saved => saved.id === c.id)}><option value="telegram">Telegram</option></select></Field>
                  <Field label="Name" hint={attempted && !c.label.trim() ? "Name is required." : undefined}>
                    <input value={c.label} aria-invalid={attempted && !c.label.trim()} onChange={e => edit(c.id, {label: e.target.value})} />
                  </Field>
                </div>
                <Toggle label="Enabled" checked={c.enabled} onChange={enabled => edit(c.id, {enabled})} />
                <div className="form-grid">
                  <Field label="Chat ID" hint={attempted && c.enabled && !c.config.chat_id.trim() ? "Chat ID is required." : "Destination chat or channel ID."}>
                    <input value={c.config.chat_id} aria-invalid={attempted && c.enabled && !c.config.chat_id.trim()} onChange={e => edit(c.id, {config: {...c.config, chat_id: e.target.value}})} />
                  </Field>
                  <Field label="Bot token" hint={attempted && c.enabled && !c.config.has_token && !c.config.bot_token?.trim() ? "Bot token is required." : "Leave blank to keep a configured token."}>
                    <input type="password" autoComplete="new-password" spellCheck={false} value={c.config.bot_token ?? ""}
                      aria-invalid={attempted && c.enabled && !c.config.has_token && !c.config.bot_token?.trim()}
                      onChange={e => edit(c.id, {config: {...c.config, bot_token: e.target.value}})} />
                  </Field>
                </div>
                <p>{c.config.has_token ? "Token configured" : "Token not configured"}</p>
                <fieldset className="notifications-events" aria-invalid={attempted && c.enabled && !c.events.length} aria-describedby={attempted && c.enabled && !c.events.length ? `events-error-${c.id}` : undefined}>
                  <legend>Run events</legend>
                  <div className="notifications-event-list">
                    {notificationEvents.map(event => <Toggle key={event.id} label={event.label} checked={c.events.includes(event.id)} onChange={checked => edit(c.id, {events: checked ? [...c.events, event.id] : c.events.filter(e => e !== event.id)})} />)}
                    {attempted && c.enabled && !c.events.length && <p id={`events-error-${c.id}`}>Select at least one run event.</p>}
                  </div>
                </fieldset>
              </> : isGoogleChatConnector(c) ? <GoogleChatConnectorEditor connector={c} profiles={settings.token_profiles} busy={busy} attempted={attempted} edit={patch => edit(c.id, patch)} /> : <p>Unsupported connector type: {c.type}</p>}
              <div className="notifications-connector-actions" role="group" aria-label="Connector actions">
                <button className="button button-secondary" onClick={() => remove(c.id)} aria-label={`Remove connector ${c.label}`}>Remove connector</button>
              </div>
            </div>
          </fieldset>)}
        </div>}
      <div className="notifications-form-actions" role="group" aria-label="Notification settings actions">
        <button className="button button-primary" disabled={!dirty || busy} onClick={() => void save()}>{busy ? "Saving…" : "Save changes"}</button>
        <button className="button button-secondary" disabled={!dirty || busy} onClick={discard}>Discard changes</button>
      </div>
    </div>
  </section>;
}
