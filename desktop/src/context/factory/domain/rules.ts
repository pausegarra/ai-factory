import type { Flow, Settings } from "./types";
export type ActivationStatus = "Enabled" | "Disabled globally" | "Disabled for this account";
export function profilePreference(flow: Flow, profileId: string): boolean {
  const map = flow.profile_activation;
  return map && Object.prototype.hasOwnProperty.call(map, profileId) ? map[profileId] : true;
}
export function activationStatus(flow: Flow, profileId: string): ActivationStatus {
  if (!flow.enabled) return "Disabled globally";
  return profilePreference(flow, profileId) ? "Enabled" : "Disabled for this account";
}
export function validateFlowActivation(flow: Flow): string[] {
  const errors: string[] = [];
  if (typeof flow.enabled !== "boolean") errors.push("Workflow activation must be a boolean.");
  const activation = flow.profile_activation;
  if (activation !== undefined) {
    if (!activation || typeof activation !== "object" || Array.isArray(activation)) {
      errors.push("Workflow profile activation must be an object.");
    } else {
      for (const [id, enabled] of Object.entries(activation)) {
        if (!/^[a-zA-Z0-9_.-]{1,128}$/.test(id) || id === "." || id === "..")
          errors.push("Invalid workflow activation profile identifier.");
        if (typeof enabled !== "boolean") errors.push("Workflow profile activation must contain booleans.");
      }
    }
  }
  return errors;
}
export function validateFlow(flow: Flow): string[] {
  const errors = validateFlowActivation(flow);
  const ids = new Set(flow.nodes.map((n) => n.id));
  if (
    flow.max_parallel !== undefined &&
    (!Number.isInteger(flow.max_parallel) ||
      flow.max_parallel < 1 ||
      flow.max_parallel > 64)
  )
    errors.push("Parallel steps must be an integer from 1 to 64.");
  if (flow.enabled && !flow.nodes.length)
    errors.push("An enabled workflow needs at least one node.");
  if (ids.size !== flow.nodes.length)
    errors.push("Node identifiers must be unique.");
  for (const n of flow.nodes) {
    if (!/^[a-zA-Z0-9_-]{1,128}$/.test(n.id))
      errors.push("Invalid node identifier.");
    if (!n.prompt.trim()) errors.push(`${n.name}: write a prompt.`);
    if (n.effort && !n.model.trim())
      errors.push(`${n.name}: choose a model to set effort.`);
    for (const [profile, config] of Object.entries(n.token_overrides ?? {})) {
      if (!/^[a-zA-Z0-9_.-]{1,128}$/.test(profile) || profile === "." || profile === "..")
        errors.push(`${n.name}: invalid token override profile identifier.`);
      if (!["codex", "opencode"].includes(config.agent))
        errors.push(`${n.name}: unsupported agent for token ${profile}.`);
      if (config.effort && !config.model.trim())
        errors.push(`${n.name}: choose a model to set effort for token ${profile}.`);
    }
    if (
      !n.result_schema ||
      typeof n.result_schema !== "object" ||
      Array.isArray(n.result_schema)
    )
      errors.push(`${n.name}: the schema must be a JSON object.`);
    for (const dep of n.depends_on)
      if (!ids.has(dep))
        errors.push(`${n.name}: dependency ${dep} does not exist.`);
  }
  const done = new Set<string>();
  let changed = true;
  while (changed) {
    changed = false;
    for (const n of flow.nodes)
      if (!done.has(n.id) && n.depends_on.every((d) => done.has(d))) {
        done.add(n.id);
        changed = true;
      }
  }
  if (
    done.size !== flow.nodes.length &&
    !errors.some((e) => e.includes("does not exist"))
  )
    errors.push("Connections form a cycle.");
  return errors;
}
export function connectNodes(flow: Flow, source: string, target: string): Flow {
  const result = {
    ...flow,
    nodes: flow.nodes.map((n) =>
      n.id === target
        ? { ...n, depends_on: [...new Set([...n.depends_on, source])] }
        : n,
    ),
  };
  const errors = validateFlow(result).filter(
    (e) => e.includes("cycle") || e.includes("does not exist"),
  );
  if (errors.length) throw new Error(errors[0]);
  return result;
}
export const removeNode = (flow: Flow, id: string): Flow => ({
  ...flow,
  nodes: flow.nodes
    .filter((n) => n.id !== id)
    .map((n) => ({ ...n, depends_on: n.depends_on.filter((d) => d !== id) })),
});
export function validateSettings(s: Settings): string[] {
  const errors: string[] = [];
  if (
    !Number.isInteger(s.max_concurrent) ||
    s.max_concurrent < 1 ||
    s.max_concurrent > 64
  )
    errors.push("Concurrency must be an integer from 1 to 64.");
  if (!Number.isInteger(s.poll_interval_seconds) || s.poll_interval_seconds < 1)
    errors.push("Poll interval must be a positive integer.");
  if (!Number.isInteger(s.node_timeout_seconds) || s.node_timeout_seconds < 1)
    errors.push("Timeout must be a positive integer.");
  if (Object.values(s.commands).some((v) => !v.trim()))
    errors.push("Enter the CLI executable paths.");
  const accounts = new Set<string>();
  for (const t of s.token_profiles) {
    if (!t.label.trim() || !t.account.trim())
      errors.push("Each token needs a name and GitHub account.");
    if (!t.has_token && !t.token?.trim())
      errors.push(`Add a token for ${t.label || "the new profile"}.`);
    if (accounts.has(t.account.toLowerCase()))
      errors.push("Only one profile per GitHub account.");
    accounts.add(t.account.toLowerCase());
  }
  for (const p of s.projects) {
    if (!/^[\w.-]+\/[\w.-]+$/.test(p.repository))
      errors.push("Repository must use owner/repository format.");
  }
  return errors;
}
