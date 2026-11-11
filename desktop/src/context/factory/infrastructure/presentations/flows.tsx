import { useEffect, useRef, useState } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  Handle,
  Position,
  MarkerType,
  useReactFlow,
  type NodeProps,
  type Node,
  type Connection,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  ArrowsOut,
  Check,
  Code,
  FloppyDisk,
  GitBranch,
  Plus,
  Robot,
  Trash,
  X,
} from "@phosphor-icons/react";
import type { Flow, FlowNode, Settings, Trigger } from "../../domain/types";
import { TRIGGERS } from "../../domain/types";
import { connectNodes, removeNode, validateFlow } from "../../domain/rules";
import { Empty, Field, Notice, Toggle } from "./ui";
import { AgentModelFields } from "./agent-model-fields";
import { TokenAgentFields } from "./token-agent-fields";
import { arrangeFlow, displayFlow, flowConnections, hasAutomaticLayout } from "../../application/flow-layout";
type AgentNode = Node<{ step: FlowNode; index: number }, "agent">;
function AgentBox({ data, selected }: NodeProps<AgentNode>) {
  return (
    <div className={`agent-node ${selected ? "selected" : ""}`}>
      <Handle type="target" position={Position.Left} />
      <div className="agent-node-top">
        <span className={`agent-symbol ${data.step.agent}`}>
          <Robot size={20} />
        </span>
        <span className="node-index">
          {String(data.index + 1).padStart(2, "0")}
        </span>
      </div>
      <strong>{data.step.name || "Untitled"}</strong>
      <span className="agent-node-sub">
        {data.step.agent === "codex" ? "Codex" : "OpenCode"}
        <span>·</span>
        {data.step.access === "write" ? "Write" : "Read"}
      </span>
      <div className="node-model">{data.step.model || "Default model"}</div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
const nodeTypes = { agent: AgentBox };
function FitArrangedFlow({ revision }: { revision: number }) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    if (!revision) return;
    const frame = requestAnimationFrame(() => { void fitView({ padding: 0.15, maxZoom: 1 }); });
    return () => cancelAnimationFrame(frame);
  }, [revision, fitView]);
  return null;
}
export function FlowsPage({
  settings,
  onSave,
  onDirtyChange,
}: {
  settings: Settings;
  onSave: (s: Settings) => Promise<void>;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [trigger, setTrigger] = useState<Trigger>("new_issue");
  const [draft, setDraft] = useState<Flow | null>(null);
  const [selected, setSelected] = useState("");
  const [selectedEdges, setSelectedEdges] = useState<string[]>([]);
  const [layoutRevision, setLayoutRevision] = useState(0);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [schemaText, setSchemaText] = useState("");
  const [schemas, setSchemas] = useState<Record<string, string>>({});
  const original = settings.flows.find((f) => f.id === trigger)!;
  const flow = displayFlow(draft?.id === trigger ? draft : original);
  // React Flow can emit node and edge removals in the same event.
  const latestFlow = useRef(flow);
  latestFlow.current = flow;
  const node = flow.nodes.find((n) => n.id === selected);
  const dirty = draft != null;
  useEffect(() => {
    onDirtyChange?.(dirty);
    return () => onDirtyChange?.(false);
  }, [dirty, onDirtyChange]);
  function update(value: Flow) {
    latestFlow.current = value;
    setDraft(value);
    setSaved(false);
  }
  function updateTopology(next: Flow) {
    if (hasAutomaticLayout(latestFlow.current)) {
      update(arrangeFlow(next));
      setLayoutRevision((value) => value + 1);
    } else update(next);
    setSelectedEdges([]);
  }
  function arrange() {
    try {
      update(arrangeFlow(flow));
      setLayoutRevision((value) => value + 1);
      setError("");
    } catch (error) { setError((error as Error).message); }
  }
  function updateNode(patch: Partial<FlowNode>) {
    update({
      ...flow,
      nodes: flow.nodes.map((n) =>
        n.id === selected ? { ...n, ...patch } : n,
      ),
    });
  }
  function select(id: string) {
    setSelectedEdges([]);
    setSelected(id);
    const n = flow.nodes.find((n) => n.id === id);
    setSchemaText(
      schemas[id] ?? JSON.stringify(n?.result_schema || {}, null, 2),
    );
    setError("");
  }
  function switchTrigger(id: Trigger) {
    if (dirty && id !== trigger) {
      setError("Save or discard your changes before switching triggers.");
      return;
    }
    setTrigger(id);
    setSelectedEdges([]);
    setDraft(null);
    setSchemas({});
    setSelected("");
    setError("");
    setSaved(false);
  }
  function add() {
    const id = `step_${crypto.randomUUID().slice(0, 8)}`;
    const next: FlowNode = {
      id,
      name: "New step",
      agent: "codex",
      model: "",
      effort: "",
      access: "write",
      prompt: "",
      result_schema: { type: "object" },
      depends_on: [],
      position: { x: 80 + flow.nodes.length * 280, y: 180 },
    };
    updateTopology({ ...flow, nodes: [...flow.nodes, next] });
    setSelected(id);
    setSchemaText(JSON.stringify(next.result_schema, null, 2));
  }
  function connect(c: Connection) {
    if (!c.source || !c.target) return;
    try {
      updateTopology(connectNodes(latestFlow.current, c.source, c.target));
      setError("");
    } catch (e) {
      setError((e as Error).message);
    }
  }
  async function save() {
    for (const n of flow.nodes) {
      if (schemas[n.id] !== undefined) {
        try {
          JSON.parse(schemas[n.id]);
        } catch {
          setError(`${n.name}: the schema needs valid JSON before saving.`);
          return;
        }
      }
    }
    const errors = validateFlow(flow);
    if (errors.length) {
      setError(errors.join(" "));
      return;
    }
    setBusy(true);
    setError("");
    try {
      await onSave({
        ...settings,
        flows: settings.flows.map((f) => (f.id === trigger ? flow : f)),
      });
      setDraft(null);
      setSchemas({});
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const nodes: AgentNode[] = flow.nodes.map((step, index) => ({
    id: step.id,
    type: "agent",
    position: step.position,
    data: { step, index },
    selected: step.id === selected,
  }));
  const edges = flowConnections(flow).map((connection) => {
    const active = selectedEdges.includes(connection.id) || connection.source === selected || connection.target === selected;
    const color = active ? "#e76a30" : "#8e8e8c";
    return {
      ...connection,
      type: "default",
      selected: selectedEdges.includes(connection.id),
      markerEnd: { type: MarkerType.ArrowClosed, color, width: 14, height: 14 },
      style: { stroke: color, strokeWidth: 2 },
      interactionWidth: 24,
    };
  });
  return (
    <>
      <div className="page-title">
        <div>
          <span className="eyebrow">DESIGN YOUR PRODUCTION LINE</span>
          <h1>Workflows</h1>
          <p>Three entry points. Your way of building.</p>
        </div>
        <div className="flex gap-3">
          {dirty && (
            <button
              className="button button-secondary"
              disabled={busy}
              onClick={() => {
                setDraft(null);
                setSchemas({});
                setSelected("");
                setError("");
              }}
            >
              Discard
            </button>
          )}
          <button
            className="button button-primary"
            disabled={busy || !dirty}
            onClick={() => void save()}
          >
            <FloppyDisk size={17} />
            {busy ? "Saving…" : "Save workflow"}
          </button>
        </div>
      </div>
      {error && <Notice close={() => setError("")}>{error}</Notice>}
      {saved && (
        <Notice tone="success" close={() => setSaved(false)}>
          Workflow saved. New tasks will use this version.
        </Notice>
      )}
      <div className="trigger-tabs">
        {Object.entries(TRIGGERS).map(([id, t], i) => (
          <button
            className={trigger === id ? "selected" : ""}
            key={id}
            onClick={() => switchTrigger(id as Trigger)}
          >
            <span className="trigger-number">0{i + 1}</span>
            <span>
              <strong>{t.label}</strong>
              <small>
                {settings.flows.find((f) => f.id === id)?.nodes.length || 0}{" "}
                {settings.flows.find((f) => f.id === id)?.nodes.length === 1
                  ? "step"
                  : "steps"}
              </small>
            </span>
            <GitBranch size={18} />
          </button>
        ))}
      </div>
      <section className={`panel flow-workspace ${node ? "inspecting" : ""}`}>
        <div className="flow-toolbar">
          <div>
            <span className="status-dot" />
            {TRIGGERS[trigger].short}
            <span className="muted">
              {flow.nodes.length} {flow.nodes.length === 1 ? "node" : "nodes"} ·{" "}
              {edges.length} {edges.length === 1 ? "connection" : "connections"}
            </span>
          </div>
          <div className="flow-toolbar-actions">
            <button className="button button-secondary" disabled={!flow.nodes.length} onClick={arrange}>
              <GitBranch size={16} />
              Arrange flow
            </button>
            <button className="button button-secondary" onClick={add}>
              <Plus size={16} />
              Add step
            </button>
          </div>
        </div>
        <div className="flow-body">
          <div className="flow-canvas">
            {flow.nodes.length ? (
              <ReactFlow
                key={trigger}
                nodes={nodes}
                edges={edges}
                nodeTypes={nodeTypes}
                onNodeClick={(_, n) => select(n.id)}
                onPaneClick={() => { setSelected(""); setSelectedEdges([]); }}
                onEdgeClick={() => setSelected("")}
                onConnect={connect}
                isValidConnection={(connection) => {
                  if (!connection.source || !connection.target || !flow.nodes.some((n) => n.id === connection.source) || !flow.nodes.some((n) => n.id === connection.target)) return false;
                  if (flow.nodes.find((n) => n.id === connection.target)?.depends_on.includes(connection.source)) return false;
                  try { connectNodes(flow, connection.source, connection.target); return true; } catch { return false; }
                }}
                onNodesChange={(changes) => {
                  const current = latestFlow.current;
                  let next = current;
                  for (const change of changes) {
                    if (change.type === "position" && change.position)
                      next = {
                        ...next,
                        nodes: next.nodes.map((n) =>
                          n.id === change.id
                            ? { ...n, position: change.position! }
                            : n,
                        ),
                      };
                    if (change.type === "remove")
                      next = removeNode(next, change.id);
                  }
                  if (next !== current) {
                    if (changes.some((change) => change.type === "remove")) updateTopology(next);
                    else update(next);
                  }
                }}
                onEdgesChange={(changes) => {
                  setSelectedEdges((previous) => {
                    const next = new Set(previous);
                    for (const change of changes) {
                      if (change.type === "select") { if (change.selected) next.add(change.id); else next.delete(change.id); }
                      if (change.type === "remove") next.delete(change.id);
                    }
                    return [...next];
                  });
                  const removed = changes
                    .filter((c) => c.type === "remove")
                    .map((c) => c.id);
                  if (removed.length) {
                    const current = latestFlow.current;
                    updateTopology({
                      ...current,
                      nodes: current.nodes.map((n) => ({
                        ...n,
                        depends_on: n.depends_on.filter(
                          (d) => !removed.includes(`${d}::${n.id}`),
                        ),
                      })),
                    });
                  }
                }}
                colorMode="dark"
                fitView
                defaultViewport={{ x: 16, y: 48, zoom: 0.85 }}
                minZoom={0.25}
                maxZoom={1.5}
                fitViewOptions={{ padding: 0.2, maxZoom: 1 }}
                deleteKeyCode={["Backspace", "Delete"]}
              >
                <Background color="#363636" gap={24} size={1} />
                <Controls showInteractive={false} />
                <FitArrangedFlow revision={layoutRevision} />
              </ReactFlow>
            ) : (
              <Empty
                icon={<GitBranch size={32} />}
                title="Your workflow starts here"
                description="Add a step, choose an agent and write its instructions. Connect outputs to set the order."
                action="Add first step"
                onAction={add}
              />
            )}
            <div className="canvas-hint">
              <ArrowsOut size={14} />
              Drag to move · Connect the dots · Delete to remove
            </div>
          </div>
          {node && (
            <aside className="node-inspector">
              <div className="inspector-heading">
                <span className="eyebrow">STEP SETTINGS</span>
                <button
                  className="icon-button"
                  aria-label="Close inspector"
                  onClick={() => setSelected("")}
                >
                  <X size={18} />
                </button>
              </div>
              <div className="inspector-content">
                <Field label="Name">
                  <input
                    value={node.name}
                    onChange={(e) => updateNode({ name: e.target.value })}
                  />
                </Field>
                <Field label="Agent">
                  <select
                    value={node.agent}
                    onChange={(e) =>
                      updateNode({
                        agent: e.target.value as "codex" | "opencode",
                        model: "",
                        effort: "",
                      })
                    }
                  >
                    <option value="codex">Codex</option>
                    <option value="opencode">OpenCode</option>
                  </select>
                </Field>
                <AgentModelFields node={node} command={settings.commands[node.agent]} onChange={updateNode} />
                <TokenAgentFields node={node} profiles={settings.token_profiles} commands={settings.commands} onChange={updateNode} />
                <Field label="Workspace access">
                  <select
                    value={node.access}
                    onChange={(e) =>
                      updateNode({ access: e.target.value as "read" | "write" })
                    }
                  >
                    <option value="read">Read only</option>
                    <option value="write">Read and write</option>
                  </select>
                </Field>
                <Field
                  label="Prompt"
                  hint="The agent also receives the event, workspace and previous results."
                >
                  <textarea
                    className="prompt-input"
                    rows={8}
                    value={node.prompt}
                    placeholder="Describe what this step should do…"
                    onChange={(e) => updateNode({ prompt: e.target.value })}
                  />
                </Field>
                <Field label="Result schema · JSON">
                  <textarea
                    className="code-input"
                    rows={6}
                    value={schemaText}
                    onChange={(e) => {
                      setSchemaText(e.target.value);
                      setSchemas((v) => ({ ...v, [selected]: e.target.value }));
                      setDraft(flow);
                      setSaved(false);
                      try {
                        const parsed = JSON.parse(e.target.value);
                        updateNode({ result_schema: parsed });
                        setError("");
                      } catch {
                        setError("The schema needs valid JSON before saving.");
                      }
                    }}
                  />
                </Field>
                <div className="node-id">
                  <Code size={14} />
                  {node.id}
                </div>
                <button
                  className="button button-danger w-full"
                  onClick={() => {
                    updateTopology(removeNode(flow, node.id));
                    setSelected("");
                  }}
                >
                  <Trash size={16} />
                  Delete step
                </button>
              </div>
            </aside>
          )}
        </div>
        <div className="flow-footer">
          <span>{TRIGGERS[trigger].description}</span>
          <label className="parallel-label">
            Parallel steps
            <input
              aria-label="Parallel steps"
              type="number"
              min={1}
              max={64}
              value={flow.max_parallel ?? 3}
              onChange={(e) =>
                update({ ...flow, max_parallel: Number(e.target.value) })
              }
            />
          </label>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={flow.enabled}
              onChange={(e) => update({ ...flow, enabled: e.target.checked })}
            />
            Trigger enabled
          </label>
        </div>
      </section>
      <p className="footnote">
        <Check size={14} />
        Changes apply to new runs. Active tasks keep their original workflow.
      </p>
    </>
  );
}
