import { useRef } from "react";
import type { CreativeNodeData, CreativePickerCategory, CreativePickerOption } from "../../types";
import { CREATIVE_NODE_PICKER_CATEGORIES } from "../../types";
import { useCreativeStore } from "../../store/creativeStore";
import { PickerIcon } from "../../components/PickerIcon";
import { importCanvasMediaFile, type CanvasMediaKind } from "../../utils/canvasMedia";

interface Props {
  id: string;
  data: CreativeNodeData;
}

function getCategory(data: CreativeNodeData): CreativePickerCategory | undefined {
  return CREATIVE_NODE_PICKER_CATEGORIES.find((item) => item.key === data.creativeCategoryKey);
}

function optionsFor(category: CreativePickerCategory): Array<{ group?: string; option: CreativePickerOption }> {
  const options = (category.options ?? []).map((option) => ({ option }));
  const grouped = (category.optionGroups ?? []).flatMap((group) =>
    group.options.map((option) => ({ group: group.label, option })),
  );
  return [...options, ...grouped];
}

function stopNodeGesture(event: React.SyntheticEvent) {
  event.stopPropagation();
}

export function CategoryPickerContent({ id, data }: Props) {
  const category = getCategory(data);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  if (!category) {
    return <div className="text-xs" style={{ color: "rgba(248,248,248,0.45)" }}>节点类型已失效</div>;
  }

  const refine = (option: CreativePickerOption, extraData: Record<string, unknown> = {}) => {
    if (option.key === "image-to-prompt") {
      useCreativeStore.getState().createImageToPromptPair(id, extraData);
      return;
    }
    useCreativeStore.getState().refineNode(id, option.kind, {
      label: option.label,
      creativeCategory: category.label,
      creativeCategoryKey: category.key,
      creativeAction: option.label,
      creativeActionKey: option.key,
      ...extraData,
    });
  };

  const handleFile = async (file: File | undefined, option: CreativePickerOption) => {
    if (!file) return;
    const mime = file.type || "";
    const mediaKind: CanvasMediaKind =
      option.key === "parse-document"
        ? "document"
        : mime.startsWith("video/")
          ? "video"
          : mime.startsWith("audio/")
            ? "audio"
            : "image";
    const kind =
      mediaKind === "document"
        ? "text-gen"
        : mediaKind === "video"
          ? "video-gen"
          : mediaKind === "audio"
            ? "audio"
            : "image-gen";
    let reference: string;
    try {
      // data URL 只是 webview 到内核的一次性载体，节点上只留 artifact 引用。
      reference = await importCanvasMediaFile(mediaKind, file);
    } catch (error) {
      console.error(`[canvas] ${file.name} 入库失败，已跳过:`, error);
      return;
    }
    const extraData: Record<string, unknown> = {
      label: file.name.replace(/\.[^.]+$/, "") || file.name,
      sourceFileName: file.name,
      mediaUrl: reference,
      mediaType: mediaKind,
    };
    if (mediaKind === "video") extraData.videoUrl = reference;
    if (mediaKind === "audio") extraData.audioReference = reference;
    if (mediaKind === "image") extraData.imageUrl = reference;
    if (kind === "text-gen") {
      extraData.prompt = `待解析文档：${file.name}`;
      if (file.type.startsWith("text/") || /\.(txt|md|markdown)$/i.test(file.name)) {
        extraData.prompt = await file.text();
      }
    }
    refine(option, extraData);
  };

  const handleOption = (option: CreativePickerOption) => {
    if (option.key === "upload-image") {
      if (fileInputRef.current) {
        fileInputRef.current.accept = "image/*";
        fileInputRef.current.value = "";
        fileInputRef.current.dataset.optionKey = option.key;
        fileInputRef.current.click();
      }
      return;
    }
    if (option.key === "parse-document") {
      if (fileInputRef.current) {
        fileInputRef.current.accept = ".txt,.md,.markdown,.doc,.docx,.pdf";
        fileInputRef.current.value = "";
        fileInputRef.current.dataset.optionKey = option.key;
        fileInputRef.current.click();
      }
      return;
    }
    refine(option, {
      ...(option.key === "text-to-video" ? { videoPrompt: "" } : {}),
      ...(option.key === "image-to-image" ? { imageInputUrls: [] } : {}),
      ...(option.key === "replace-background" ? { editMode: "remove-bg" } : {}),
    });
  };

  return (
    <>
      <input
        ref={fileInputRef}
        type="file"
        className="hidden"
        onChange={(event) => {
          const optionKey = event.currentTarget.dataset.optionKey;
          const option = optionsFor(category).find((item) => item.option.key === optionKey)?.option;
          handleFile(event.currentTarget.files?.[0], option ?? { key: "upload", label: "上传", kind: "image-gen" });
        }}
      />
      <div className="flex flex-col gap-1">
        <div className="mb-1 text-[10px]" style={{ color: "rgba(248,248,248,0.38)" }}>
          选择具体类型后完成节点细化
        </div>
        {optionsFor(category).map(({ group, option }) => (
          <div key={option.key}>
            {group && (
              <div className="px-1 pb-1 pt-2 text-[10px] font-semibold tracking-wide" style={{ color: "rgba(248,248,248,0.42)" }}>
                {group}
              </div>
            )}
            <button
              type="button"
              className="group flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left transition hover:bg-white/[0.09]"
              onPointerDown={stopNodeGesture}
              onMouseDown={stopNodeGesture}
              onClick={() => handleOption(option)}
            >
              <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md" style={{ background: "rgba(255,255,255,0.055)", color: "rgba(248,248,248,0.72)" }}>
                <PickerIcon name={option.icon ?? category.icon} size={13} />
              </span>
              <span className="min-w-0 flex-1 truncate text-xs" style={{ color: "rgba(248,248,248,0.87)" }}>
                {option.label}
              </span>
              <PickerIcon name="arrow-right" size={14} className="opacity-0 transition group-hover:opacity-60" style={{ color: "rgba(248,248,248,0.72)" }} />
            </button>
          </div>
        ))}
      </div>
    </>
  );
}
