import { sourceLabel } from "../../domain/source-connectors";
import type { Agent, AgentConfig, FlowNode, Settings } from "../../domain/types";
import { AgentModelFields } from "./agent-model-fields";
import { Field } from "./ui";

export function TokenAgentFields({ node, sources, commands, bindings = {}, onChange }: {
  node: FlowNode;
  sources: Settings["source_connectors"];
  commands: Settings["commands"];
  bindings?: Settings["codex_bindings"];
  onChange: (patch: Partial<FlowNode>) => void;
}) {
  const overrides = node.token_overrides ?? {};
  const unavailable = Object.keys(overrides).filter((id) => !sources.some((source) => source.id === id));

  function update(source: string, config?: AgentConfig) {
    const next = config ? { ...overrides, [source]: config } : { ...overrides };
    if (!config) delete next[source];
    onChange({ token_overrides: next });
  }

  return (
    <section className="token-agent-fields" aria-label="GitHub token overrides">
      <h3>GitHub token overrides</h3>
      <p>Optional for this step. Each token uses the step defaults unless you choose an agent below.</p>
      {!sources.length && <p>Add a GitHub Issues connector in Sources to choose a different configuration for this step.</p>}
      {sources.map((source) => {
        const config = Object.hasOwn(overrides, source.id) ? overrides[source.id] : undefined;
        return (
          <fieldset key={source.id}>
            <legend>{sourceLabel(source, sources)}</legend>
            <Field label={`Agent for ${sources.filter(p => p.label === source.label).length > 1 ? sourceLabel(source, sources) : source.label}`}>
              <select value={config?.agent ?? ""} onChange={(event) => update(source.id, event.target.value ? {
                agent: event.target.value as Agent,
                model: "",
                effort: "",
              } : undefined)}>
                <option value="">Use step defaults</option>
                <option value="codex">Codex</option>
                <option value="opencode">OpenCode</option>
              </select>
            </Field>
            {config && <AgentModelFields node={config} command={commands[config.agent]} sourceConnectorId={source.id} boundAccountId={Object.hasOwn(bindings, source.id) ? bindings[source.id] : undefined} onChange={(patch) => update(source.id, { ...config, ...patch })} />}
          </fieldset>
        );
      })}
      {unavailable.map((id) => (
        <fieldset key={id}>
          <legend>Unavailable source: {id}</legend>
          <p>This override is inactive because its GitHub token source was removed.</p>
          <button type="button" className="text-button" onClick={() => update(id)} aria-label={`Remove override for ${id}`}>Remove override</button>
        </fieldset>
      ))}
    </section>
  );
}
