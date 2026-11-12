import type { Flow, TokenProfile } from "../../domain/types";
import { activationStatus, profilePreference } from "../../domain/rules";

export function WorkflowActivationFields({ flow, profiles, onChange }: {
  flow: Flow;
  profiles: TokenProfile[];
  onChange: (flow: Flow) => void;
}) {
  const present = new Set(profiles.map((profile) => profile.id));
  const unavailable = Object.keys(flow.profile_activation ?? {}).filter((id) => !present.has(id));
  return (
    <section className="panel settings-section token-agent-fields" aria-label={`Activation for ${flow.id}`}>
      <label className="checkbox-label">
        <input type="checkbox" checked={flow.enabled}
          aria-label={`Enable workflow globally for ${flow.id}`}
          onChange={(event) => onChange({ ...flow, enabled: event.target.checked })} />
        Enable workflow globally
      </label>
      <h2>GitHub accounts</h2>
      <p>Changes take effect after saving. Applies to new runs and retries. Queued and running tasks continue. Use Cancel to stop an existing task.</p>
      {!profiles.length && <p>Add GitHub accounts in Settings. The global workflow control remains available.</p>}
      {profiles.map((profile) => (
        <fieldset key={profile.id} aria-label={`Activation for ${flow.id}: ${profile.label} (@${profile.account})`}>
          <legend>{profile.label} (@{profile.account})</legend>
          <label className="checkbox-label">
            <input type="checkbox" checked={profilePreference(flow, profile.id)}
              aria-label={`Enable ${flow.id} for ${profile.label} (@${profile.account})`}
              onChange={(event) => onChange({ ...flow, profile_activation: {
                ...flow.profile_activation, [profile.id]: event.target.checked,
              } })} />
            Allow new runs for this account
          </label>
          <p role="status">{activationStatus(flow, profile.id)}</p>
          {!profile.has_token && <p>No token saved. Activation does not confirm a working connection.</p>}
        </fieldset>
      ))}
      {unavailable.map((id) => (
        <fieldset key={id} aria-label={`Unavailable activation profile: ${id}`}>
          <legend>Unavailable profile: {id}</legend>
          <p>This preference has no effect until this profile ID exists.</p>
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
