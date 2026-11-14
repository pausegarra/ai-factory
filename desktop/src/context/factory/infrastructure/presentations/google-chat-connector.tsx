import type { GoogleChatNotificationConnector, NotificationConnector, TokenProfile, Trigger } from "../../domain/types";
import { TRIGGERS } from "../../domain/types";
import { notificationEvents, validWebhookUrl } from "../../domain/notification-connectors";
import { Field, Toggle } from "./ui";
export function GoogleChatConnectorEditor({connector: c, profiles, busy, attempted, edit}: {
  connector: GoogleChatNotificationConnector;
  profiles: TokenProfile[];
  busy: boolean;
  attempted: boolean;
  edit: (patch: Partial<NotificationConnector>) => void;
}) {
  const required = attempted && c.enabled;
  const rawUrl = c.config.webhook_url ?? "";
  const invalidUrl = (!!rawUrl.trim() && !validWebhookUrl(rawUrl)) || (c.enabled && !rawUrl.trim() && !c.config.has_webhook_url);
  const missingProfiles = !c.config.token_profile_ids.length || c.config.token_profile_ids.some(id => !profiles.some(p => p.id === id));
  const choose = <T extends string,>(items: T[], item: T, checked: boolean) => checked ? [...items, item] : items.filter(i => i !== item);
  const profileLabel = (p: TokenProfile) => `${p.label || p.account} (@${p.account})`;
  return <>
    <div className="form-grid">
      <Field label="Connector type"><select value="google_chat" disabled><option value="google_chat">Google Chat</option></select></Field>
      <Field label="Name" hint={attempted && !c.label.trim() ? "Name is required." : undefined}><input value={c.label} aria-invalid={attempted && !c.label.trim()} onChange={e => edit({label: e.target.value})} /></Field>
    </div>
    <Toggle label="Enabled" checked={c.enabled} onChange={enabled => edit({enabled})} />
    <Field label="Webhook URL" hint={attempted && invalidUrl ? (rawUrl.trim() ? "Enter a valid HTTPS webhook URL without credentials or a fragment." : "Webhook URL is required.") : "Leave blank to keep a configured webhook."}>
      <input type="password" autoComplete="new-password" spellCheck={false} value={rawUrl} disabled={busy} aria-invalid={attempted && invalidUrl} onChange={e => edit({config: {...c.config, webhook_url: e.target.value}})} />
    </Field>
    <p>{c.config.has_webhook_url ? "Webhook configured" : "Webhook not configured"}</p>
    <fieldset className="notifications-events" aria-invalid={required && !c.events.length} aria-describedby={required && !c.events.length ? `events-error-${c.id}` : undefined}>
      <legend>Run events</legend><div className="notifications-event-list">
        {notificationEvents.map(e => <Toggle key={e.id} label={e.label} checked={c.events.includes(e.id)} onChange={checked => edit({events: choose(c.events, e.id, checked)})} />)}
        {required && !c.events.length && <p id={`events-error-${c.id}`}>Select at least one run event.</p>}
      </div>
    </fieldset>
    <fieldset className="notifications-events" aria-invalid={required && !c.config.triggers.length} aria-describedby={required && !c.config.triggers.length ? `triggers-error-${c.id}` : undefined}>
      <legend>GitHub triggers</legend><div className="notifications-event-list">
        {(Object.keys(TRIGGERS) as Trigger[]).map(id => <Toggle key={id} label={TRIGGERS[id].label} checked={c.config.triggers.includes(id)} onChange={checked => edit({config: {...c.config, triggers: choose(c.config.triggers, id, checked)}})} />)}
        {required && !c.config.triggers.length && <p id={`triggers-error-${c.id}`}>Select at least one GitHub trigger.</p>}
      </div>
    </fieldset>
    <fieldset className="notifications-events" aria-invalid={required && missingProfiles} aria-describedby={required && missingProfiles ? `profiles-error-${c.id}` : undefined}>
      <legend>GitHub profiles</legend><div className="notifications-event-list">
        {!profiles.length && <p>Add a GitHub profile in Settings to enable this connector.</p>}
        {profiles.map(p => <Toggle key={p.id} label={profileLabel(p) + (profiles.filter(other => profileLabel(other) === profileLabel(p)).length > 1 ? ` (${p.id})` : "")} checked={c.config.token_profile_ids.includes(p.id)} onChange={checked => edit({config: {...c.config, token_profile_ids: choose(c.config.token_profile_ids, p.id, checked)}})} />)}
        {c.config.token_profile_ids.filter(id => !profiles.some(p => p.id === id)).map(id => <Toggle key={id} label={`Unavailable profile (${id})`} checked onChange={() => edit({config: {...c.config, token_profile_ids: c.config.token_profile_ids.filter(p => p !== id)}})} />)}
        {required && missingProfiles && <p id={`profiles-error-${c.id}`}>Select available GitHub profiles.</p>}
      </div>
    </fieldset>
  </>;
}
