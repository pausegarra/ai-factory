import { useEffect, useId, useRef, useState } from "react";
import { Bell } from "@phosphor-icons/react";
import type { NotificationConnector, Settings } from "../../domain/types";
import { cleanNotificationConnectors, isTelegramConnector, isGoogleChatConnector, newGoogleChatConnector, newTelegramConnector, validateNotificationConnectors } from "../../domain/notification-connectors";
import { Empty, Notice } from "./ui";
import { ConnectorPicker, type ConnectorKind } from "./connector-picker";
import { NotificationConnectorDialog } from "./notification-connector-dialog";
import { NotificationConnectorIcon } from "./notification-connector-icon";

type ConnectorEditSession = {
  mode: "new" | "existing";
  original: NotificationConnector;
  value: NotificationConnector;
};

// Refresh public credential presence without touching pending replacement secrets.
function refreshCredentialHints(items: NotificationConnector[], incoming: NotificationConnector[]): NotificationConnector[] {
  return items.map(c => {
    if (isGoogleChatConnector(c)) {
      const persisted = incoming.find(saved => saved.id === c.id && isGoogleChatConnector(saved));
      return {...c, config: {...c.config, has_webhook_url: !!persisted && isGoogleChatConnector(persisted) && persisted.config.has_webhook_url}};
    }
    if (isTelegramConnector(c)) {
      const persisted = incoming.find(saved => saved.id === c.id && isTelegramConnector(saved));
      return {...c, config: {...c.config, has_token: !!persisted && isTelegramConnector(persisted) && persisted.config.has_token}};
    }
    return c;
  });
}

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
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const [session, setSession] = useState<ConnectorEditSession | null>(null);
  const dialogId = useId();
  const deferredIncoming = useRef<NotificationConnector[] | null>(null);
  const focusRow = useRef<string | null>(null);
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
    const sessionChanged = session && JSON.stringify(session.value) !== JSON.stringify(session.original);
    if (!dirty && !sessionChanged) {
      setBaseline(incoming); setDraft(incoming); deferredIncoming.current = null;
      if (session?.mode === "existing") {
        const updated = incoming.find(c => c.id === session.value.id);
        setSession(updated ? {mode: "existing", original: structuredClone(updated), value: structuredClone(updated)} : null);
      }
    } else {
      if (!dirty) deferredIncoming.current = incoming;
      setBaseline(items => refreshCredentialHints(items, incoming));
      setDraft(items => refreshCredentialHints(items, incoming));
      setSession(current => current ? {...current,
        original: refreshCredentialHints([current.original], incoming)[0],
        value: refreshCredentialHints([current.value], incoming)[0],
      } : null);
    }
  }, [settings]);
  useEffect(() => { onDirtyChange?.(dirty); return () => onDirtyChange?.(false); }, [dirty, onDirtyChange]);
  useEffect(() => { onBusyChange?.(busy); return () => onBusyChange?.(false); }, [busy, onBusyChange]);
  function openExisting(c: NotificationConnector) {
    if (inFlight.current || session) return;
    focusRow.current = c.id;
    setSession({mode: "existing", original: structuredClone(c), value: structuredClone(c)});
  }
  function add(kind: ConnectorKind) {
    if (inFlight.current || session) return;
    const id = crypto.randomUUID();
    const c = kind === "google_chat" ? newGoogleChatConnector(id) : newTelegramConnector(id);
    focusRow.current = null;
    setSession({mode: "new", original: structuredClone(c), value: structuredClone(c)});
  }
  function editSession(patch: Partial<NotificationConnector>) {
    if (inFlight.current) return;
    setSession(current => current ? {...current, value: {...current.value, ...patch}} : null);
  }
  function applySession() {
    if (!session || inFlight.current) return;
    const value = structuredClone(session.value);
    setDraft(items => session.mode === "new" ? [...items, value] : items.map(c => c.id === value.id ? value : c));
    setError(""); setSaved(false); deferredIncoming.current = null; setSession(null);
  }
  function cancelSession() {
    if (inFlight.current) return;
    if (!dirty && deferredIncoming.current) {
      setBaseline(structuredClone(deferredIncoming.current)); setDraft(structuredClone(deferredIncoming.current));
    }
    deferredIncoming.current = null; setSession(null);
  }
  function remove(id: string) {
    if (inFlight.current) return;
    const remaining = draft.filter(c => c.id !== id);
    const index = draft.findIndex(c => c.id === id);
    focusRow.current = remaining[Math.min(index, remaining.length - 1)]?.id ?? null;
    setDraft(remaining); setSaved(false); setError(""); deferredIncoming.current = null; setSession(null);
  }
  function discard() {
    if (inFlight.current) return;
    const clean = publicConnectors(); setBaseline(clean); setDraft(clean);
    deferredIncoming.current = null; setSession(null);
    setError(""); setSaved(false); setAttempted(false);
  }
  async function save() {
    if (inFlight.current || session) return;
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
    if (errors.length) {
      setError(errors.join(" "));
      const invalid = draft.find(c => validateNotificationConnectors([c], settings.token_profiles).length);
      if (invalid) openExisting(invalid);
      return;
    }
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
      if (current()) { deferredIncoming.current = null; setDraft(clean); setBaseline(clean); setSaved(true); setAttempted(false); }
    } catch {
      if (current()) setError("Could not save notification settings. Check your configuration and try again.");
    } finally {
      if (current()) { inFlight.current = false; setBusy(false); }
    }
  }
  return <><section inert={!!session} className="notifications-page" aria-labelledby="notifications-title">
    <div className="page-title">
      <div><h1 id="notifications-title">Notifications</h1><p>Choose which run updates each connector sends.</p></div>
      <ConnectorPicker disabled={busy || !!session} buttonRef={addButton} onSelect={add} />
    </div>
    <div className="notifications-content">
      {error && <Notice>{error}</Notice>}
      {saved && <Notice tone="success">Notification settings saved.</Notice>}
      {!draft.length ? <Empty icon={<Bell size={28} />} title="No notification connectors" description="Add a connector to receive run updates outside AI Factory." /> :
        <ul className="notifications-connector-list" aria-label="Notification connectors">
          {draft.map(c => <li key={c.id}>
            <button type="button" className="notifications-connector-row"
              ref={element => { if (element) rows.current.set(c.id, element); else rows.current.delete(c.id); }}
              disabled={busy || !!session} aria-label={`Edit connector ${c.label || "Connector"}`}
              aria-haspopup="dialog" aria-expanded={session?.value.id === c.id}
              aria-controls={session?.value.id === c.id ? dialogId : undefined} onClick={() => openExisting(c)}>
              <NotificationConnectorIcon type={c.type} /><span>{c.label || "Connector"}</span>
            </button>
          </li>)}
        </ul>}
      <div className="notifications-form-actions" role="group" aria-label="Notification settings actions">
        <button className="button button-primary" disabled={!dirty || busy || !!session} onClick={() => void save()}>{busy ? "Saving…" : "Save changes"}</button>
        <button className="button button-secondary" disabled={!dirty || busy || !!session} onClick={discard}>Discard changes</button>
      </div>
    </div>
  </section>
    {session && <NotificationConnectorDialog id={dialogId} connector={session.value} mode={session.mode}
      profiles={settings.token_profiles} attempted={attempted} busy={busy} onChange={editSession}
      onApply={applySession} onCancel={cancelSession}
      onRemove={session.mode === "existing" ? () => remove(session.value.id) : undefined}
      returnFocus={() => (focusRow.current ? rows.current.get(focusRow.current) : null) ?? addButton.current} />}
  </>;
}
