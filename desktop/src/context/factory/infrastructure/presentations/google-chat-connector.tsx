import type { GoogleChatNotificationConnector, NotificationConnector, TokenProfile } from "../../domain/types";
import { validWebhookUrl } from "../../domain/notification-connectors";
import { NotificationConnectorFilters } from "./notification-connector-filters";
import { Field, Toggle } from "./ui";
export function GoogleChatConnectorEditor({connector: c, profiles, busy, attempted, edit}: {
  connector: GoogleChatNotificationConnector;
  profiles: TokenProfile[];
  busy: boolean;
  attempted: boolean;
  edit: (patch: Partial<NotificationConnector>) => void;
}) {
  const rawUrl = c.config.webhook_url ?? "";
  const invalidUrl = (!!rawUrl.trim() && !validWebhookUrl(rawUrl)) || (c.enabled && !rawUrl.trim() && !c.config.has_webhook_url);
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
    <NotificationConnectorFilters
      id={c.id} enabled={c.enabled} events={c.events}
      triggers={c.config.triggers} tokenProfileIds={c.config.token_profile_ids}
      profiles={profiles} attempted={attempted}
      onChange={({events, triggers, token_profile_ids}) => edit({events, config: {...c.config, triggers, token_profile_ids}})}
    />
  </>;
}
