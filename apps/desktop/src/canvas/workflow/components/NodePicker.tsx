import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Connection } from "@xyflow/react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import {
  CREATIVE_NODE_PICKER_CATEGORIES,
  PROFESSIONAL_NODE_PICKER_TABS,
  type CreativePickerCategory,
  type CreativeNodeKind,
  type ProfessionalNodeKind,
} from "../types";
import { PickerIcon } from "./PickerIcon";

const ICON_COLOR = "rgba(248,248,248,0.72)";

const CREATIVE_PICKER_GROUPS = [
  { label: "基础节点", keys: ["text", "image", "video", "camera", "director", "compare", "template", "character"] },
  { label: "媒体节点", keys: ["audio", "storyboard"] },
  { label: "添加资源", keys: ["upload"] },
] as const;

export function NodePicker() {
  const mode = useCanvasStore((s) => s.mode);
  const nodePicker = useCanvasStore((s) => s.nodePicker);
  const pendingConnection = useCanvasStore((s) => s.pendingConnection);
  const closeNodePicker = useCanvasStore((s) => s.closeNodePicker);
  const addCreativeNode = useCreativeStore((s) => s.addNode);
  const addProfessionalNode = useProfessionalStore((s) => s.addNode);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [uploadAccept, setUploadAccept] = useState("image/*,video/*,audio/*");
  const [uploadContext, setUploadContext] = useState<{
    category: string;
    action: string;
    actionKey: string;
  } | null>(null);
  const [activeTab, setActiveTab] = useState(0);

  useEffect(() => {
    if (!nodePicker) {
      setUploadContext(null);
      setActiveTab(0);
    }
  }, [nodePicker]);

  useLayoutEffect(() => {
    const el = panelRef.current;
    if (!el || !nodePicker) return;
    const { width, height } = el.getBoundingClientRect();
    const margin = 8;
    el.style.left = `${Math.max(margin, Math.min(nodePicker.x, window.innerWidth - width - margin))}px`;
    el.style.top = `${Math.max(margin, Math.min(nodePicker.y, window.innerHeight - height - margin))}px`;
  }, [nodePicker]);

  const connectNewNode = (newNodeId: string) => {
    if (!pendingConnection) return;
    const connection: Connection =
      pendingConnection.handleType === "source"
        ? { source: pendingConnection.nodeId, target: newNodeId, sourceHandle: null, targetHandle: null }
        : { source: newNodeId, target: pendingConnection.nodeId, sourceHandle: null, targetHandle: null };
    useCreativeStore.getState().onConnect(connection);
  };

  const createCreativeNode = (kind: CreativeNodeKind, extraData: Record<string, unknown> = {}) => {
    if (!nodePicker) return;
    const newNodeId = addCreativeNode(kind, nodePicker.worldX, nodePicker.worldY);
    if (Object.keys(extraData).length > 0) useCreativeStore.getState().updateNodeData(newNodeId, extraData);
    connectNewNode(newNodeId);
    closeNodePicker();
  };

  const createCategoryNode = (category: CreativePickerCategory) => {
    if (!nodePicker) return;
    const newNodeId = useCreativeStore.getState().addNodeAt("category-picker", nodePicker.worldX, nodePicker.worldY, {
      label: category.label,
      creativeCategory: category.label,
      creativeCategoryKey: category.key,
      creativeAction: "选择具体类型",
      creativeActionKey: "category-picker",
    });
    connectNewNode(newNodeId);
    closeNodePicker();
  };

  const openUpload = (accept: string, context: { category: string; action: string; actionKey: string }) => {
    setUploadAccept(accept);
    setUploadContext(context);
    if (fileInputRef.current) {
      fileInputRef.current.value = "";
      fileInputRef.current.click();
    }
  };

  const handleFiles = (files: FileList | null) => {
    if (!nodePicker || !files || files.length === 0) return;
    const context = uploadContext ?? { category: "上传", action: "本地文件", actionKey: "upload" };
    Array.from(files).forEach((file, index) => {
      const url = URL.createObjectURL(file);
      const mime = file.type || "";
      const kind: CreativeNodeKind = context.actionKey === "parse-document"
        ? "text-gen"
        : mime.startsWith("video/")
          ? "video-gen"
          : mime.startsWith("audio/")
            ? "audio"
            : "image-gen";
      const extraData: Record<string, unknown> = {
        label: file.name.replace(/\.[^.]+$/, "") || file.name,
        creativeCategory: context.category,
        creativeAction: context.action,
        creativeActionKey: context.actionKey,
        sourceFileName: file.name,
        mediaUrl: url,
        mediaType: kind === "video-gen" ? "video" : kind === "audio" ? "audio" : "image",
      };
      if (kind === "video-gen") extraData.videoUrl = url;
      if (kind === "image-gen") extraData.imageUrl = url;
      if (kind === "text-gen") extraData.prompt = `待解析文档：${file.name}`;
      const newNodeId = useCreativeStore.getState().addNode(kind, nodePicker.worldX + index * 264, nodePicker.worldY + index * 24);
      useCreativeStore.getState().updateNodeData(newNodeId, extraData);
      if (index === 0) connectNewNode(newNodeId);
    });
    closeNodePicker();
  };

  const handleCreativeCategory = (category: CreativePickerCategory) => {
    if (category.action === "upload") {
      openUpload("image/*,video/*,audio/*", { category: category.label, action: category.label, actionKey: category.key });
      return;
    }
    if (category.directKind) {
      createCreativeNode(category.directKind, {
        creativeCategory: category.label,
        creativeAction: category.label,
        creativeActionKey: category.key,
      });
      return;
    }
    createCategoryNode(category);
  };

  const handleProfessionalSelect = (kind: ProfessionalNodeKind) => {
    if (!nodePicker) return;
    const newNodeId = addProfessionalNode(kind, nodePicker.worldX, nodePicker.worldY);
    if (pendingConnection) {
      const connection: Connection =
        pendingConnection.handleType === "source"
          ? { source: pendingConnection.nodeId, target: newNodeId, sourceHandle: null, targetHandle: null }
          : { source: newNodeId, target: pendingConnection.nodeId, sourceHandle: null, targetHandle: null };
      useProfessionalStore.getState().onConnect(connection);
    }
    closeNodePicker();
  };

  if (!nodePicker) return null;

  const panelTitle = "添加节点";
  const panelSubtitle = "选择一个流程节点";

  return (
    <>
      <div className="wf-floating-layer fixed inset-0 z-[9998]" onClick={closeNodePicker} />
      <div
        ref={panelRef}
        className="wf-floating-layer fixed z-[9999] overflow-hidden rounded-2xl"
        style={{
          left: nodePicker.x,
          top: nodePicker.y,
          width: mode === "creative" ? 336 : 300,
          background: "rgba(29,30,33,0.96)",
          border: "1px solid rgba(255,255,255,0.09)",
          backdropFilter: "blur(36px)",
          WebkitBackdropFilter: "blur(40px)",
          boxShadow: "0 18px 48px rgba(0,0,0,0.48), 0 0 0 1px rgba(255,255,255,0.02)",
        }}
      >
        <input
          ref={fileInputRef}
          type="file"
          multiple
          accept={uploadAccept}
          className="hidden"
          onChange={(event) => handleFiles(event.target.files)}
        />
        <div className="flex items-center gap-2 px-3.5 py-3" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
          <div className="min-w-0">
            <div className="text-[13px] font-semibold tracking-wide" style={{ color: "rgba(248,248,248,0.92)" }}>{panelTitle}</div>
            {panelSubtitle && <div className="mt-0.5 truncate text-[10px]" style={{ color: "rgba(248,248,248,0.38)" }}>{panelSubtitle}</div>}
          </div>
        </div>

        {mode === "creative" && (
          <div className="node-picker-scroll max-h-[min(650px,78vh)] overflow-y-auto px-2 py-2">
            {CREATIVE_PICKER_GROUPS.map((group) => (
              <section key={group.label} className="mb-2 last:mb-0">
                <div className="px-2 pb-1.5 pt-1 text-[10px] font-semibold tracking-[0.08em]" style={{ color: "rgba(248,248,248,0.38)" }}>
                  {group.label}
                </div>
                <div className="space-y-0.5">
                  {group.keys.map((key) => {
                    const category = CREATIVE_NODE_PICKER_CATEGORIES.find((item) => item.key === key);
                    return category ? <PickerCategoryButton key={category.key} category={category} onClick={() => handleCreativeCategory(category)} /> : null;
                  })}
                </div>
              </section>
            ))}
          </div>
        )}

        {mode === "professional" && (
          <>
            <div className="flex gap-0.5 px-2 pt-2">
              {PROFESSIONAL_NODE_PICKER_TABS.map((tab, i) => (
                <button
                  key={tab.label}
                  onClick={() => setActiveTab(i)}
                  className="rounded-md px-3 py-1.5 text-xs font-medium transition-colors"
                  style={{
                    background: activeTab === i ? "rgba(255,255,255,0.12)" : "transparent",
                    color: activeTab === i ? "rgba(248,248,248,0.92)" : "rgba(248,248,248,0.45)",
                  }}
                >
                  {tab.label}
                </button>
              ))}
            </div>
            <div className="node-picker-scroll max-h-80 overflow-y-auto overscroll-contain p-1.5">
              {(() => {
                const tab = PROFESSIONAL_NODE_PICKER_TABS[activeTab];
                if (!tab.groups.length) {
                  return (
                    <div className="flex flex-col items-center justify-center py-8">
                      <span className="text-xs" style={{ color: "rgba(248,248,248,0.3)" }}>
                        {tab.label}
                      </span>
                      <span className="mt-1 text-[10px]" style={{ color: "rgba(248,248,248,0.2)" }}>
                        即将开放
                      </span>
                    </div>
                  );
                }
                return tab.groups.map((group) => (
                  <div key={group.label ?? "_"} className="mb-1">
                    {group.label && (
                      <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider" style={{ color: "rgba(248,248,248,0.38)" }}>
                        {group.label}
                      </div>
                    )}
                    {group.items.map((item) => (
                      <button
                        key={item.kind}
                        draggable
                        onDragStart={(e) => {
                          e.dataTransfer.setData("application/workflow-node-kind", item.kind);
                          e.dataTransfer.effectAllowed = "copy";
                        }}
                        onClick={() => handleProfessionalSelect(item.kind)}
                        className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-all duration-150 hover:bg-white/10"
                      >
                        <PickerIcon name={item.icon} size={14} style={{ color: ICON_COLOR, width: 16 }} />
                        <span className="text-xs" style={{ color: "rgba(248,248,248,0.85)" }}>{item.label}</span>
                      </button>
                    ))}
                  </div>
                ));
              })()}
            </div>
          </>
        )}
      </div>
    </>
  );
}

function PickerCategoryButton({
  category,
  onClick,
}: {
  category: CreativePickerCategory;
  onClick: () => void;
}) {
  const hasChildren = Boolean(category.options?.length || category.optionGroups?.length);
  return (
    <button
      type="button"
      onClick={onClick}
      className="group flex min-h-[52px] w-full items-center gap-3 rounded-xl px-2.5 py-2 text-left transition duration-150 hover:bg-white/[0.075]"
    >
      <span
        className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg transition duration-150 group-hover:bg-white/[0.1]"
        style={{ background: "rgba(255,255,255,0.055)", color: ICON_COLOR }}
      >
        <PickerIcon name={category.icon} size={16} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-[13px] font-medium" style={{ color: "rgba(248,248,248,0.86)" }}>{category.label}</span>
        <span className="mt-0.5 block truncate text-[10px]" style={{ color: "rgba(248,248,248,0.36)" }}>{category.description}</span>
      </span>
      {hasChildren && (
        <PickerIcon
          name="chevron-right"
          className="opacity-35 transition duration-150 group-hover:translate-x-0.5 group-hover:opacity-80"
          size={14}
          strokeWidth={1.8}
          style={{ color: ICON_COLOR }}
        />
      )}
    </button>
  );
}
