import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { fileViewerRenderers } from "@file-viewer/vite-plugin";

// fileViewerRenderers injects its renderer-registration script into every HTML
// entry. The startup splash never previews files, so strip that script there to
// keep the splash bundle to React + GSAP.
function stripFileViewerFromSplash(): Plugin {
  return {
    name: "deepagent:strip-file-viewer-from-splash",
    transformIndexHtml: {
      order: "pre",
      handler(html, ctx) {
        if (!ctx.filename.replace(/\\/g, "/").endsWith("/splash.html")) return html;
        return html.replace(/<script[^>]*src="[^"]*file-viewer[^"]*"[^>]*><\/script>\s*/g, "");
      },
    },
  };
}

// Tauri expects a fixed dev port and a relative base for the bundled assets.
export default defineConfig(({ command }) => ({
  plugins: [
    react(),
    // dev 期 file-viewer 资产已由 package.json 的 predev（file-viewer-copy-assets）
    // 预先复制到 public/file-viewer，插件再复制一遍是重复写入约 2900 个文件，
    // 实测拖慢 dev server 启动约 20 秒。build 期没有 predev，仍交给插件复制。
    fileViewerRenderers({ copyAssets: command === "build" }),
    stripFileViewerFromSplash(),
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
    rollupOptions: {
      // splash.html is the startup splash window (tauri.conf.json "splashscreen").
      // A separate entry keeps its bundle to React + GSAP instead of the whole app.
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        splash: fileURLToPath(new URL("./splash.html", import.meta.url)),
      },
      output: {
        // Pin the libraries both windows share into their own chunks. Without
        // this, Rollup folds React into whichever file-viewer renderer chunk
        // first claims it, and any entry that imports React (the splash, and
        // the main window too) then statically loads every renderer chunk.
        manualChunks(id: string) {
          const normalized = id.replace(/\\/g, "/");
          if (/\/node_modules\/(\.pnpm\/[^/]+\/node_modules\/)?(react|react-dom|scheduler)\//.test(normalized)) {
            return "vendor-react";
          }
          if (/\/node_modules\/(\.pnpm\/[^/]+\/node_modules\/)?gsap\//.test(normalized)) {
            return "vendor-gsap";
          }
          if (/\/node_modules\/(\.pnpm\/[^/]+\/node_modules\/)?@tauri-apps\/api\//.test(normalized)) {
            return "vendor-tauri";
          }
          return undefined;
        },
      },
    },
  },
}));
