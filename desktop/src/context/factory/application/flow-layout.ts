import type { Flow, FlowNode } from "../domain/types";

export function flowConnections(flow: Flow) {
  return flow.nodes.flatMap((node) =>
    [...new Set(node.depends_on)].map((source) => ({
      id: `${source}::${node.id}`,
      source,
      target: node.id,
    })),
  );
}

/** Longest dependency path determines columns; siblings occupy separate rows. */
export function arrangeFlow(flow: Flow): Flow {
  if (!flow.nodes.length) return flow;
  const ids = new Set(flow.nodes.map((node) => node.id));
  if (ids.size !== flow.nodes.length) throw new Error("Node identifiers must be unique.");
  const depth = new Map<string, number>();
  while (depth.size < flow.nodes.length) {
    const before = depth.size;
    for (const node of flow.nodes) {
      if (node.depends_on.some((id) => !ids.has(id)))
        throw new Error("A connection endpoint does not exist.");
      if (!depth.has(node.id) && node.depends_on.every((id) => depth.has(id)))
        depth.set(node.id, Math.max(-1, ...node.depends_on.map((id) => depth.get(id)!)) + 1);
    }
    if (depth.size === before) throw new Error("Connections form a cycle.");
  }
  const columns = new Map<number, FlowNode[]>();
  for (const node of flow.nodes) {
    const column = depth.get(node.id)!;
    columns.set(column, [...(columns.get(column) ?? []), node]);
  }
  const rows = Math.max(...[...columns.values()].map((column) => column.length));
  return {
    ...flow,
    nodes: flow.nodes.map((node) => {
      const column = depth.get(node.id)!;
      const siblings = columns.get(column)!;
      return {
        ...node,
        position: {
          x: 80 + column * (216 + 64),
          y: 80 + ((rows - siblings.length) / 2 + siblings.indexOf(node)) * (152 + 64),
        },
      };
    }),
  };
}

function isLegacyRow(flow: Flow) {
  return flow.nodes.every((node, index) => node.position.x === 80 + index * 280 && node.position.y === 180);
}

export function hasAutomaticLayout(flow: Flow): boolean {
  if (isLegacyRow(flow)) return true;
  try {
    return arrangeFlow(flow).nodes.every((node, index) =>
      node.position.x === flow.nodes[index].position.x && node.position.y === flow.nodes[index].position.y,
    );
  } catch {
    return false;
  }
}

/** Migrate the old generated row visually without changing manually saved layouts. */
export function displayFlow(flow: Flow): Flow {
  if (!isLegacyRow(flow)) return flow;
  try { return arrangeFlow(flow); } catch { return flow; }
}
