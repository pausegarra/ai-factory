import { sourceLabel } from "../../domain/source-connectors";
import type { NotificationEvent, SourceConnector, Trigger } from "../../domain/types";
import { TRIGGERS } from "../../domain/types";
import { notificationEvents } from "../../domain/notification-connectors";
import { Toggle } from "./ui";

export interface NotificationFilterSelection {
  events: NotificationEvent[];
  triggers: Trigger[];
  token_profile_ids: string[];
}
export interface NotificationConnectorFiltersProps {
  id: string;
  enabled: boolean;
  events: NotificationEvent[];
  triggers: Trigger[];
  tokenProfileIds: string[];
  sources: SourceConnector[];
  attempted: boolean;
  onChange: (selection: NotificationFilterSelection) => void;
}
export function NotificationConnectorFilters({id, enabled, events, triggers, tokenProfileIds, sources, attempted, onChange}: NotificationConnectorFiltersProps) {
  const required = attempted && enabled;
  const missingProfiles = !tokenProfileIds.length || tokenProfileIds.some(id => !sources.some(p => p.id === id));
  const choose = <T extends string,>(items: T[], item: T, checked: boolean): T[] => checked ? (items.includes(item) ? items : [...items, item]) : items.filter(i => i !== item);
  const emit = (patch: Partial<NotificationFilterSelection>) => onChange({events, triggers, token_profile_ids: tokenProfileIds, ...patch});
  return <>
    <fieldset className="notifications-events" aria-invalid={required && !events.length} aria-describedby={required && !events.length ? `events-error-${id}` : undefined}>
      <legend>Run events</legend><div className="notifications-event-list">
        {notificationEvents.map(e => <Toggle key={e.id} label={e.label} checked={events.includes(e.id)} onChange={checked => emit({events: choose(events, e.id, checked)})} />)}
        {required && !events.length && <p id={`events-error-${id}`}>Select at least one run event.</p>}
      </div>
    </fieldset>
    <fieldset className="notifications-events" aria-invalid={required && !triggers.length} aria-describedby={required && !triggers.length ? `triggers-error-${id}` : undefined}>
      <legend>GitHub triggers</legend><div className="notifications-event-list">
        {(Object.keys(TRIGGERS) as Trigger[]).map(id => <Toggle key={id} label={TRIGGERS[id].label} checked={triggers.includes(id)} onChange={checked => emit({triggers: choose(triggers, id, checked)})} />)}
        {required && !triggers.length && <p id={`triggers-error-${id}`}>Select at least one GitHub trigger.</p>}
      </div>
    </fieldset>
    <fieldset className="notifications-events" aria-invalid={required && missingProfiles} aria-describedby={required && missingProfiles ? `sources-error-${id}` : undefined}>
      <legend>GitHub sources</legend><div className="notifications-event-list">
        {!sources.length && <p>Add a GitHub Issues connector in Sources to enable this connector.</p>}
        {sources.map(p => <Toggle key={p.id} label={sourceLabel(p, sources)} checked={tokenProfileIds.includes(p.id)} onChange={checked => emit({token_profile_ids: choose(tokenProfileIds, p.id, checked)})} />)}
        {tokenProfileIds.filter(id => !sources.some(p => p.id === id)).map(id => <Toggle key={id} label={`Unavailable source (${id})`} checked onChange={() => emit({token_profile_ids: tokenProfileIds.filter(p => p !== id)})} />)}
        {required && missingProfiles && <p id={`sources-error-${id}`}>Select available Source connectors.</p>}
      </div>
    </fieldset>
  </>;
}
