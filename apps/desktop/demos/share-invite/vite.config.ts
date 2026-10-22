import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
  base: "./",
  plugins: [react()],
  // This preview has its own stylesheet; do not inherit the desktop Tailwind build.
  css: { postcss: { plugins: [] } },
  server: { host: "127.0.0.1", port: 5174, strictPort: true },
  build: { target: "es2020", outDir: "dist", emptyOutDir: true },
});
