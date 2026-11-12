import type { NodeRun, RunDetail } from "../domain/types";

/** Display precedence only; never change a run snapshot or scheduler input. */
export function orderRunSteps(detail: Pick<RunDetail, "flow" | "nodes">): NodeRun[] {
  const { nodes } = detail;
  const fallback = () => nodes.slice();
  if (nodes.length === 0) return [];

  const definitions = detail.flow.nodes;
  if (definitions.length !== nodes.length) return fallback();
  const byId = new Map(nodes.map(node => [node.id, node]));
  const definitionIds = new Set(definitions.map(node => node.id));
  if (byId.size !== nodes.length || definitionIds.size !== definitions.length)
    return fallback();
  if (definitions.some(node => !byId.has(node.id))) return fallback();

  const indegree = new Map<string, number>();
  const successors = new Map<string, string[]>();
  for (const node of definitions) {
    const dependencies = new Set(node.depends_on);
    if ([...dependencies].some(id => !definitionIds.has(id))) return fallback();
    indegree.set(node.id, dependencies.size);
    for (const dependency of dependencies) {
      const next = successors.get(dependency) ?? [];
      next.push(node.id);
      successors.set(dependency, next);
    }
  }

  const emitted = new Set<string>();
  const ordered: NodeRun[] = [];
  while (ordered.length < definitions.length) {
    const next = definitions.find(node => !emitted.has(node.id) && indegree.get(node.id) === 0);
    if (!next) return fallback();
    emitted.add(next.id);
    ordered.push(byId.get(next.id)!);
    for (const successor of successors.get(next.id) ?? [])
      indegree.set(successor, indegree.get(successor)! - 1);
  }
  return ordered;
}
