# DeepAgent Studio — Desktop UI

A Codex-style desktop UI for the DeepAgent runtime, built on **Tauri v2 + React +
TypeScript + Vite**.

## Layout (Codex-style)

```text
┌───────────────────────────────────────────────────────────┐
│  DeepAgent.Studio                      [⌘K]      [tauri]    │  top bar
├────────────┬───────────────────────────────┬──────────────┤
│ Sessions   │  Session title + id           │  Metrics     │
│ (sidebar)  │  Agent timeline (replayable)  │  (inspector) │
│            │                               │              │
└────────────┴───────────────────────────────┴──────────────┘
   ⌘K Command Palette · ⌘D Diff View · Approval Dialog overlays
```

- **Sidebar** — session list with relative time + active/ended badges.
- **Main** — the replayable Agent Timeline (icons, labels, durations) built from
  the append-only event log.
- **Inspector** — live session metrics (events, messages, tool calls, success
  rate, durations); toggle with the palette.
- **Command Palette** (⌘K / Ctrl+K) — fuzzy-filtered command list served by
  `app-core::commands`; arrow-key navigation, Enter to run.
- **Diff View** (⌘D / Ctrl+D) — side-by-side editor + unified diff computed by
  `app-core::diff` (real LCS diff in Rust, mirrored in TS for preview).
- **Approval Dialog** — high-risk tool gate: shows tool + args + reason with
  Approve/Reject (driven by the runtime's `WaitingApproval` state).

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
- ✅ Interactive components: Command Palette (⌘K), Diff View (⌘D), Approval
  Dialog — wired to Tauri commands (`commands`, `compute_diff`) over
  `deepagent-app-core`.
- ⏳ `pnpm tauri dev`/`build` require the platform Tauri prerequisites
  (Rust + WebView2 on Windows / WebKitGTK on Linux / WKWebView on macOS). A
  placeholder icon ships at `src-tauri/icons/icon.png`; replace it with real
  multi-resolution icons (`pnpm tauri icon`) before distribution. The
  `src-tauri` crate is intentionally outside the Cargo workspace so the kernel
  workspace stays lean.
