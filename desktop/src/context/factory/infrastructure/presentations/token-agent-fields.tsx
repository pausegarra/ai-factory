import type { Agent, AgentConfig, FlowNode, Settings } from "../../domain/types";
import { AgentModelFields } from "./agent-model-fields";
import { Field } from "./ui";

export function TokenAgentFields({ node, profiles, commands, bindings = {}, onChange }: {
  node: FlowNode;
  profiles: Settings["token_profiles"];
  commands: Settings["commands"];
  bindings?: Settings["codex_bindings"];
  onChange: (patch: Partial<FlowNode>) => void;
}) {
  const overrides = node.token_overrides ?? {};
  const unavailable = Object.keys(overrides).filter((id) => !profiles.some((profile) => profile.id === id));

  function update(profile: string, config?: AgentConfig) {
    const next = config ? { ...overrides, [profile]: config } : { ...overrides };
    if (!config) delete next[profile];
    onChange({ token_overrides: next });
  }

  return (
    <section className="token-agent-fields" aria-label="GitHub token overrides">
      <h3>GitHub token overrides</h3>
      <p>Optional for this step. Each token uses the step defaults unless you choose an agent below.</p>
      {!profiles.length && <p>Add GitHub token profiles in Settings to choose a different configuration for this step.</p>}
      {profiles.map((profile) => {
        const config = Object.hasOwn(overrides, profile.id) ? overrides[profile.id] : undefined;
        return (
          <fieldset key={profile.id}>
            <legend>{profile.label} (@{profile.account})</legend>
            <Field label={`Agent for ${profile.label}`}>
              <select value={config?.agent ?? ""} onChange={(event) => update(profile.id, event.target.value ? {
                agent: event.target.value as Agent,
                model: "",
                effort: "",
              } : undefined)}>
                <option value="">Use step defaults</option>
                <option value="codex">Codex</option>
                <option value="opencode">OpenCode</option>
              </select>
            </Field>
            {config && <AgentModelFields node={config} command={commands[config.agent]} tokenProfileId={profile.id} boundAccountId={Object.hasOwn(bindings, profile.id) ? bindings[profile.id] : undefined} onChange={(patch) => update(profile.id, { ...config, ...patch })} />}
          </fieldset>
        );
      })}
      {unavailable.map((id) => (
        <fieldset key={id}>
          <legend>Unavailable profile: {id}</legend>
          <p>This override is inactive because its GitHub token profile was removed.</p>
          <button type="button" className="text-button" onClick={() => update(id)} aria-label={`Remove override for ${id}`}>Remove override</button>
        </fieldset>
      ))}
    </section>
  );
}
