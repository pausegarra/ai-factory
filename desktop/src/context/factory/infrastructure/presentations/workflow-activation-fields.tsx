import { sourceLabel } from "../../domain/source-connectors";
import type { Flow, SourceConnector } from "../../domain/types";
import { activationStatus, profilePreference } from "../../domain/rules";

export function WorkflowActivationFields({ flow, sources, onChange }: {
  flow: Flow;
  sources: SourceConnector[];
  onChange: (flow: Flow) => void;
}) {
  const present = new Set(sources.map((source) => source.id));
  const unavailable = Object.keys(flow.profile_activation ?? {}).filter((id) => !present.has(id));
  return (
    <section className="panel settings-section token-agent-fields" aria-label={`Activation for ${flow.id}`}>
      <label className="checkbox-label">
        <input type="checkbox" checked={flow.enabled}
          aria-label={`Enable workflow globally for ${flow.id}`}
          onChange={(event) => onChange({ ...flow, enabled: event.target.checked })} />
        Enable workflow globally
      </label>
      <h2>Source connectors</h2>
      <p>Changes take effect after saving. Applies to new runs and retries. Queued and running tasks continue. Use Cancel to stop an existing task.</p>
      {!sources.length && <p>Add a GitHub Issues connector in Sources. The global workflow control remains available.</p>}
      {sources.map((source) => (
        <fieldset key={source.id} aria-label={`Activation for ${flow.id}: ${sourceLabel(source, sources)}`}>
          <legend>{sourceLabel(source, sources)}</legend>
          <label className="checkbox-label">
            <input type="checkbox" checked={profilePreference(flow, source.id)}
              aria-label={`Enable ${flow.id} for ${sourceLabel(source, sources)}`}
              onChange={(event) => onChange({ ...flow, profile_activation: {
                ...flow.profile_activation, [source.id]: event.target.checked,
              } })} />
            Allow new runs for this account
          </label>
          <p role="status">{activationStatus(flow, source.id)}</p>
          {!source.has_token && <p>No token saved. Activation does not confirm a working connection.</p>}
        </fieldset>
      ))}
      {unavailable.map((id) => (
        <fieldset key={id} aria-label={`Unavailable activation source: ${id}`}>
          <legend>Unavailable source: {id}</legend>
          <p>This preference has no effect while this source is unavailable.</p>
          <button type="button" className="button button-secondary"
            aria-label={`Remove activation preference for ${id}`}
            onClick={() => {
              const next = { ...flow.profile_activation };
              delete next[id];
              onChange({ ...flow, profile_activation: next });
            }}>Remove preference</button>
        </fieldset>
      ))}
      <p>Reactivating discovers work that is still eligible. Events no longer observable on GitHub are not replayed.</p>
    </section>
  );
}
