// Dev-only regression fixture: real canvas nodes and editor panels, no backend or model calls.
import React from "react";
import { createRoot } from "react-dom/client";
import { Background, ReactFlow } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import "../src/icons";
import "../src/styles.css";
import { Button } from "../src/components/shadcn/button";
import { WorkflowNodeShell } from "../src/canvas/workflow/components/WorkflowNodeShell";
import { useCreativeStore } from "../src/canvas/workflow/store/creativeStore";
import { useCanvasSettingsStore } from "../src/canvas/workflow/store/canvasSettingsStore";
import type { CreativeNodeData } from "../src/canvas/workflow/types";

useCanvasSettingsStore.setState({
  loaded: true,
  providers: [{
    id: "fixture", name: "验证供应商", protocol: "openai", baseUrl: "", apiKeySet: false,
    models: [
      { id: "text-a", name: "文本模型 A", enabled: true, scenarios: ["text"] },
      { id: "text-b", name: "文本模型 B", enabled: true, scenarios: ["text"] },
      { id: "image-a", name: "图片模型 A", enabled: true, scenarios: ["image_generation"] },
      { id: "image-b", name: "图片模型 B", enabled: true, scenarios: ["image_generation"] },
    ],
  }],
});
useCreativeStore.setState({
  nodes: [{
    id: "verification", type: "creative-text-gen", position: { x: 80, y: 120 }, selected: true,
    data: { kind: "text-gen", label: "文本节点", status: "idle", prompt: "", model: "fixture::text-a" },
  }],
  edges: [],
});
const nodeTypes = { "creative-text-gen": WorkflowNodeShell, "creative-image-gen": WorkflowNodeShell };

function Verification() {
  const { nodes, edges, onNodesChange } = useCreativeStore();
  const data = nodes[0].data as CreativeNodeData;
  const selectKind = (image: boolean, imageToPrompt = false) => {
    useCreativeStore.setState({ nodes: [{
      ...nodes[0], selected: true, type: image ? "creative-image-gen" : "creative-text-gen",
      data: {
        kind: image ? "image-gen" : "text-gen", status: "idle", label: image ? "图片节点" : "文本节点",
        prompt: "", model: "fixture::text-a", imageModel: "fixture::image-a",
        creativeActionKey: imageToPrompt ? "image-to-prompt" : undefined,
      },
    }] });
  };
  return (
    <div className="h-screen text-white" style={{ background: "#0a0a0a" }}>
      <div className="absolute left-4 right-4 top-4 z-10 flex flex-wrap items-center gap-3 text-xs">
        <Button variant="secondary" onClick={() => selectKind(false)}>文本节点</Button>
        <Button variant="secondary" onClick={() => selectKind(false, true)}>图片反推文本</Button>
        <Button variant="secondary" onClick={() => selectKind(true)}>图片节点</Button>
        <span role="status">已保存模型：{data.kind === "image-gen" ? data.imageModel : data.model}</span>
      </div>
      <ReactFlow className="studio-infinite-canvas studio-workflow-canvas" nodes={nodes} edges={edges}
        nodeTypes={nodeTypes} onNodesChange={onNodesChange} defaultViewport={{ x: 130, y: 100, zoom: 1 }}>
        <Background color="#333" gap={24} />
      </ReactFlow>
    </div>
  );
}

createRoot(document.querySelector("#root")!).render(<Verification />);
