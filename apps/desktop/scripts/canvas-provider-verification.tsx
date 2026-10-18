// Real settings UI with synthetic model assets and Tauri's official IPC mock.
import React, { Profiler } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import "../src/icons";
import "../src/styles.css";
import { ModelSettingsTab } from "../src/canvas/workflow/components/settings/ModelSettingsTab";
import { useCanvasSettingsStore, type ModelProvider } from "../src/canvas/workflow/store/canvasSettingsStore";

let reads = 0, writes = 0, clickAt = 0;
const samples: unknown[] = [];
mockIPC((command, args) => {
  if (command === "canvas_provider_save") {
    writes++;
    return { ...(args as { provider: object }).provider, apiKeySet: false };
  }
  if (command === "canvas_model_remove") { writes++; return; }
  reads++;
  throw new Error(`Unexpected verification IPC: ${command}`);
});
useCanvasSettingsStore.setState({ loaded: true, providers: [2, 270].map((count, index): ModelProvider => ({
  id: `fixture-${index}`, name: index ? "270 模型供应商" : "2 模型供应商", protocol: "openai",
  baseUrl: "https://example.invalid/v1", apiKeySet: false,
  models: Array.from({ length: count }, (_, modelIndex) => ({
    id: `provider-${index}-model-${modelIndex}`, name: `验证模型 ${modelIndex + 1}`, enabled: true,
    scenarios: modelIndex === 0 ? ["text", "image_generation", "video_generation", "speech_to_text", "text_to_speech", "embedding"] : [],
  })),
})) });
function onRender(_id: string, phase: string, actualDuration: number) {
  const clicked = clickAt;
  requestAnimationFrame(() => {
    samples.push({ phase, renderMs: Math.round(actualDuration), clickToFrameMs: clicked ? Math.round(performance.now() - clicked) : null,
      reads, writes, domElements: document.querySelectorAll("*").length,
      renderedRows: document.querySelectorAll('[role="listitem"]').length,
      previewCards: document.querySelectorAll('[data-slot="hover-card-trigger"]').length });
    document.querySelector("#metrics")!.textContent = JSON.stringify(samples.slice(-5), null, 2);
  });
}
createRoot(document.querySelector("#root")!).render(
  <div style={{ height: "100vh", background: "#0d0d0d", color: "white" }} onClickCapture={() => { clickAt = performance.now(); }}>
    <Profiler id="provider-settings" onRender={onRender}><ModelSettingsTab /></Profiler>
    <details style={{ position: "fixed", right: 8, bottom: 8, background: "#111", color: "#8f8", padding: 8, fontSize: 10, zIndex: 30000 }}>
      <summary>性能采样</summary><pre id="metrics" style={{ maxHeight: 240, overflow: "auto" }} />
    </details>
  </div>
);
