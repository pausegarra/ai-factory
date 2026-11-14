import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: { port: 1420, strictPort: true },
  test: {
    environment: "jsdom",
    clearMocks: true,
    // Preserve the raw stylesheet used by the Capacity presentation test.
    css: { include: [/\/src\/styles\.css\?raw$/] },
  },
});
