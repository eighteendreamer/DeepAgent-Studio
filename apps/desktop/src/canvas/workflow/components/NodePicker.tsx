import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { Connection } from "@xyflow/react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import {
  CREATIVE_NODE_PICKER_CATEGORIES,
  PROFESSIONAL_NODE_CATEGORIES,
  type CreativePickerCategory,
  type CreativePickerOption,
  type CreativeNodeKind,
  type ProfessionalNodeKind,
} from "../types";

type PickerView = { category: CreativePickerCategory } | null;

const ICON_COLOR = "rgba(248,248,248,0.72)";
const ACCENT = "#9b8afb";

function icon(name: string | undefined, fallback = "circle") {
  return ["fas", name || fallback] as IconProp;
}

export function NodePicker() {
  const mode = useCanvasStore((s) => s.mode);
  const nodePicker = useCanvasStore((s) => s.nodePicker);
  const pendingConnection = useCanvasStore((s) => s.pendingConnection);
  const closeNodePicker = useCanvasStore((s) => s.closeNodePicker);
  const addCreativeNode = useCreativeStore((s) => s.addNode);
  const addProfessionalNode = useProfessionalStore((s) => s.addNode);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [view, setView] = useState<PickerView>(null);
  const [uploadAccept, setUploadAccept] = useState("image/*,video/*,audio/*");
  const [uploadContext, setUploadContext] = useState<{
    category: string;
    action: string;
    actionKey: string;
  } | null>(null);

  useEffect(() => {
    if (!nodePicker) {
      setView(null);
      setUploadContext(null);
    }
  }, [nodePicker]);

  useLayoutEffect(() => {
    const el = panelRef.current;
    if (!el || !nodePicker) return;
    const { width, height } = el.getBoundingClientRect();
    const margin = 8;
    el.style.left = `${Math.max(margin, Math.min(nodePicker.x, window.innerWidth - width - margin))}px`;
    el.style.top = `${Math.max(margin, Math.min(nodePicker.y, window.innerHeight - height - margin))}px`;
  }, [nodePicker, view]);

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

  const handleCreativeOption = (category: CreativePickerCategory, option: CreativePickerOption) => {
    if (option.key === "upload-image") {
      openUpload("image/*", { category: category.label, action: option.label, actionKey: option.key });
      return;
    }
    if (option.key === "parse-document") {
      openUpload(".txt,.md,.markdown,.doc,.docx,.pdf", { category: category.label, action: option.label, actionKey: option.key });
      return;
    }
    createCreativeNode(option.kind, {
      creativeCategory: category.label,
      creativeAction: option.label,
      creativeActionKey: option.key,
      ...(option.key === "text-to-video" ? { videoPrompt: "" } : {}),
      ...(option.key === "image-to-image" ? { imageInputUrls: [] } : {}),
      ...(option.key === "replace-background" ? { editMode: "remove-bg" } : {}),
    });
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
    setView({ category });
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

  const creativeCategory = view?.category;
  const panelTitle = mode === "creative" ? creativeCategory?.label ?? "添加节点" : "添加节点";
  const panelSubtitle = mode === "creative" ? creativeCategory?.description : "选择一个流程节点";

  return (
    <>
      <div className="wf-floating-layer fixed inset-0 z-[9998]" onClick={closeNodePicker} />
      <div
        ref={panelRef}
        className="wf-floating-layer fixed z-[9999] overflow-hidden rounded-2xl"
        style={{
          left: nodePicker.x,
          top: nodePicker.y,
          width: mode === "creative" && creativeCategory?.optionGroups ? 360 : 300,
          background: "rgba(30,30,35,0.93)",
          border: "1px solid rgba(255,255,255,0.1)",
          backdropFilter: "blur(40px)",
          WebkitBackdropFilter: "blur(40px)",
          boxShadow: "0 24px 64px rgba(0,0,0,0.42)",
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
        <div className="flex items-center gap-2 px-3 py-2.5" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
          {mode === "creative" && creativeCategory && (
            <button
              type="button"
              className="flex h-6 w-6 items-center justify-center rounded-md text-white/60 transition hover:bg-white/10 hover:text-white"
              onClick={() => setView(null)}
              aria-label="返回节点类型"
            >
              <FontAwesomeIcon icon={icon("chevron-left")} style={{ fontSize: 11 }} />
            </button>
          )}
          <div className="min-w-0">
            <div className="text-xs font-semibold" style={{ color: "rgba(248,248,248,0.9)" }}>{panelTitle}</div>
            {panelSubtitle && <div className="mt-0.5 truncate text-[10px]" style={{ color: "rgba(248,248,248,0.42)" }}>{panelSubtitle}</div>}
          </div>
        </div>

        {mode === "creative" && !creativeCategory && (
          <div className="grid max-h-[min(540px,70vh)] grid-cols-2 gap-1.5 overflow-y-auto p-2">
            {CREATIVE_NODE_PICKER_CATEGORIES.map((category) => (
              <button
                key={category.key}
                type="button"
                onClick={() => handleCreativeCategory(category)}
                className="group flex min-h-[70px] flex-col items-start justify-between rounded-xl px-3 py-2.5 text-left transition duration-150 hover:bg-white/[0.08]"
                style={{ border: "1px solid rgba(255,255,255,0.045)" }}
              >
                <span className="flex w-full items-center justify-between">
                  <FontAwesomeIcon icon={icon(category.icon)} style={{ color: ACCENT, fontSize: 14 }} />
                  {(category.options || category.optionGroups) && <FontAwesomeIcon icon={icon("arrow-right")} className="opacity-0 transition group-hover:opacity-70" style={{ color: ICON_COLOR, fontSize: 10 }} />}
                </span>
                <span className="mt-2 text-xs font-medium" style={{ color: "rgba(248,248,248,0.86)" }}>{category.label}</span>
              </button>
            ))}
          </div>
        )}

        {mode === "creative" && creativeCategory && (
          <div className="max-h-[min(560px,72vh)] overflow-y-auto p-2">
            {creativeCategory.options?.map((option) => (
              <PickerOptionButton key={option.key} option={option} onClick={() => handleCreativeOption(creativeCategory, option)} />
            ))}
            {creativeCategory.optionGroups?.map((group) => (
              <section key={group.key} className="mb-2 last:mb-0">
                <div className="px-2 pb-1 pt-1 text-[10px] font-semibold tracking-wide" style={{ color: ACCENT }}>{group.label}</div>
                <div className="grid grid-cols-2 gap-1">
                  {group.options.map((option) => (
                    <PickerOptionButton key={option.key} option={option} compact onClick={() => handleCreativeOption(creativeCategory, option)} />
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}

        {mode === "professional" && (
          <div className="max-h-80 overflow-y-auto overscroll-contain p-1.5">
            {PROFESSIONAL_NODE_CATEGORIES.map((cat) => (
              <div key={cat.group} className="mb-1">
                <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider" style={{ color: cat.color }}>{cat.group}</div>
                {cat.items.map((item) => (
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
                    <FontAwesomeIcon icon={icon(item.icon)} style={{ fontSize: 12, color: cat.color, width: 16 }} />
                    <span className="text-xs" style={{ color: "rgba(248,248,248,0.85)" }}>{item.label}</span>
                  </button>
                ))}
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}

function PickerOptionButton({
  option,
  compact = false,
  onClick,
}: {
  option: CreativePickerOption;
  compact?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      draggable
      onDragStart={(event) => {
        event.dataTransfer.setData("application/workflow-node-kind", option.kind);
        event.dataTransfer.setData("application/workflow-node-action", JSON.stringify({ key: option.key, label: option.label }));
        event.dataTransfer.effectAllowed = "copy";
      }}
      onClick={onClick}
      className={`group flex w-full items-center gap-2 rounded-lg text-left transition-all duration-150 hover:bg-white/[0.09] ${compact ? "px-2 py-1.5" : "px-2.5 py-2"}`}
    >
      <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md" style={{ background: "rgba(155,138,251,0.12)", color: ACCENT }}>
        <FontAwesomeIcon icon={icon(option.icon, "circle-dot")} style={{ fontSize: 11 }} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-xs" style={{ color: "rgba(248,248,248,0.87)" }}>{option.label}</span>
        {option.description && <span className="mt-0.5 block truncate text-[10px]" style={{ color: "rgba(248,248,248,0.38)" }}>{option.description}</span>}
      </span>
      <FontAwesomeIcon icon={icon("arrow-right")} className="opacity-0 transition group-hover:opacity-60" style={{ color: ICON_COLOR, fontSize: 10 }} />
    </button>
  );
}
