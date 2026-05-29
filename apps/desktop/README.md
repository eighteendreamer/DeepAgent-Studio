# DeepAgent Studio — Desktop UI

A Codex-style desktop UI for the DeepAgent runtime, built on **Tauri v2 + React +
TypeScript + Vite**.

## Layout (Codex-style)

```text
┌───────────────────────────────────────────────────────────┐
│  DeepAgent.Studio                                  [tauri]  │  top bar
├────────────┬───────────────────────────────┬──────────────┤
│ Sessions   │  Session title + id           │  Metrics     │
│ (sidebar)  │  Agent timeline (replayable)  │  (inspector) │
│            │                               │              │
└────────────┴───────────────────────────────┴──────────────┘
```

- **Sidebar** — session list with relative time + active/ended badges.
- **Main** — the replayable Agent Timeline (icons, labels, durations) built from
  the append-only event log.
- **Inspector** — live session metrics (events, messages, tool calls, success
  rate, durations).

## Architecture

The UI talks to the Rust kernel through a thin contract:

```text
React (api.ts)  ──invoke──▶  Tauri commands (src-tauri)  ──▶  deepagent-app-core::AppService  ──▶  kernel
```

`deepagent-app-core` exposes serializable DTOs, so the UI never depends on
kernel internals. When run outside Tauri (plain `vite dev` or the static build),
`api.ts` falls back to deterministic mock data so the UI is always runnable.

## Develop

```bash
pnpm install
pnpm dev          # browser preview with mock data (http://localhost:1420)
pnpm build        # type-check + production bundle (verified in CI)
pnpm tauri dev    # full desktop app (requires the Tauri toolchain + system webview)
```

## Build status

- ✅ Frontend builds (`pnpm build`: tsc typecheck + vite bundle).
- ⏳ `pnpm tauri dev`/`build` require the platform Tauri prerequisites
  (Rust + WebView2 on Windows / WebKitGTK on Linux / WKWebView on macOS) and an
  app icon under `src-tauri/icons/`. The `src-tauri` crate is intentionally
  outside the Cargo workspace so the kernel workspace stays lean.
