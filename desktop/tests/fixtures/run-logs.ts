export const jsonLine = (value: unknown): string => JSON.stringify(value);

export const openText = (text: string) => ({
  type: "text",
  sessionID: "s-test",
  part: { type: "text", text },
});

export const codexText = (text: string) => ({
  type: "item.completed",
  item: { type: "agent_message", text },
});

export const command = {
  type: "item.completed",
  item: {
    id: "cmd-test",
    type: "command_execution",
    command: "npm run test",
    aggregated_output: "Tests: 3 passed\n",
    exit_code: 0,
  },
};

export const tool = {
  type: "tool_use",
  part: {
    type: "tool",
    tool: "read",
    state: {
      status: "completed",
      input: { path: "README.md" },
      output: "Project instructions",
    },
  },
};

export const envelope = {
  status: "succeeded",
  summary: "Especificacion lista",
  artifacts: [],
  data: { approved: true },
};
