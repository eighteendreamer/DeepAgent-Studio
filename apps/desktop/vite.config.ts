import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileViewerRenderers } from "@file-viewer/vite-plugin";

// Tauri expects a fixed dev port and a relative base for the bundled assets.
export default defineConfig(({ command }) => ({
  plugins: [
    react(),
    // dev 期 file-viewer 资产已由 package.json 的 predev（file-viewer-copy-assets）
    // 预先复制到 public/file-viewer，插件再复制一遍是重复写入约 2900 个文件，
    // 实测拖慢 dev server 启动约 20 秒。build 期没有 predev，仍交给插件复制。
    fileViewerRenderers({ copyAssets: command === "build" }),
  ],
  base: "./",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // public/file-viewer 是 file-viewer 的纯静态产物（约 2900 个文件 / 152MB），
      // 不在模块图内（HMR 日志全为 "no modules matched"）。一旦被覆盖写入，
      // chokidar 会逐个派发 file change，实测阻塞 dev server 启动约 74 秒。
      ignored: ["**/public/file-viewer/**"],
    },
  },
  build: {
    target: "es2020",
    outDir: "dist",
    emptyOutDir: true,
  },
}));
