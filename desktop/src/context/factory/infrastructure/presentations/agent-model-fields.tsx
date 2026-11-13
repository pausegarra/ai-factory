import { useEffect, useState } from "react";
import type { AgentConfig, AgentModel } from "../../domain/types";
import { factoryApi } from "../ipc/factory-api";
import { Field } from "./ui";
import { noticeText } from "./notice-content";

export function AgentModelFields({ node, command, tokenProfileId, boundAccountId, onChange }: {
  node: AgentConfig;
  command: string;
  tokenProfileId?: string;
  boundAccountId?: string;
  onChange: (patch: Partial<AgentConfig>) => void;
}) {
  const key = JSON.stringify([node.agent, command, tokenProfileId ?? null, node.agent === "codex" ? boundAccountId ?? null : null]);
  const [revision, setRevision] = useState(0);
  const [catalog, setCatalog] = useState<{ key: string; models: AgentModel[]; loading: boolean; error: string }>({ key: "", models: [], loading: true, error: "" });
  useEffect(() => {
    let active = true;
    setCatalog({ key, models: [], loading: true, error: "" });
    void factoryApi.agentModels(node.agent, tokenProfileId).then(
      (models) => { if (active) setCatalog({ key, models, loading: false, error: "" }); },
      (error: unknown) => { if (active) setCatalog({ key, models: [], loading: false, error: noticeText(`Could not load ${node.agent === "codex" ? "Codex" : "OpenCode"} models.`, error) }); },
    );
    return () => { active = false; };
  }, [key, node.agent, tokenProfileId, revision]);
  const loading = catalog.key !== key || catalog.loading;
  const models = catalog.key === key ? catalog.models : [];
  const currentError = catalog.key === key && !catalog.loading ? catalog.error : "";
  const model = models.find((m) => m.id === node.model);
  const efforts = model?.efforts ?? [];
  const agentName = node.agent === "codex" ? "Codex" : "OpenCode";
  return (
    <>
      <Field label="Model" hint="CLI default uses the agent's configured model.">
        <select value={node.model} disabled={loading} aria-busy={loading} onChange={(event) => onChange({ model: event.target.value, effort: "" })}>
          <option value="">CLI default</option>
          {node.model && !model && <option value={node.model} disabled>Saved model: {node.model}{loading ? "" : " (not available)"}</option>}
          {models.map((m) => <option key={m.id} value={m.id}>{node.agent === "opencode" ? `${m.name} (${m.id})` : m.name}</option>)}
        </select>
      </Field>
      <div className="model-catalog-status">
        <span role={currentError ? "alert" : "status"}>
          {loading ? `Loading ${agentName} models…` : currentError || (models.length ? `${models.length} models from ${agentName}.` : `No models available. Check ${agentName} provider configuration.`)}
        </span>
        <button type="button" className="text-button" disabled={loading} onClick={() => setRevision((value) => value + 1)} aria-label="Refresh agent models">{currentError ? "Retry" : "Refresh"}</button>
      </div>
      <Field label="Effort" hint={!node.model ? "Select a model to choose effort." : efforts.length ? (node.agent === "opencode" ? "Variants supported by this OpenCode model." : "Reasoning efforts supported by this Codex model.") : "This model uses its default effort."}>
        <select value={node.effort} disabled={loading || !node.model || (efforts.length === 0 && !node.effort)} onChange={(event) => onChange({ effort: event.target.value })}>
          <option value="">Default</option>
          {node.effort && !efforts.includes(node.effort) && <option value={node.effort} disabled>Saved effort: {node.effort} (not available)</option>}
          {efforts.map((effort) => <option key={effort} value={effort}>{effort}</option>)}
        </select>
      </Field>
    </>
  );
}
