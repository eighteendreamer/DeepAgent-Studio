import { memo, useState, useEffect } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { PreviewResult } from "../../types";
import { pickPreviewFile, previewOpenFile, previewReadDataUrl } from "../../api";
import type { PluginDefinition } from "./pluginTypes";
import { convertFileSrc } from "@tauri-apps/api/core";

/** Human-readable size. */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Icon hint per classified kind. */
function kindIcon(kind: string): IconProp {
  switch (kind) {
    case "image":
      return ["far", "image"];
    case "pdf":
      return ["far", "file-pdf"];
    case "docx":
      return ["far", "file-word"];
    case "xlsx":
      return ["far", "file-excel"];
    case "pptx":
      return ["far", "file-powerpoint"];
    case "csv":
      return ["fas", "table"];
    default:
      return ["far", "file-lines"];
  }
}

// 预览器配置保持引用稳定，避免工作区拖拽导致 file-viewer React wrapper
// 反复调用 controller.update()，从而重建 renderer 并产生闪烁。
const FILE_PREVIEW_VIEWER_OPTIONS = {
  theme: "light",
  rendererMode: "replace",
  // 使用宿主样式覆盖 renderer 的默认卡片版式。
  styleIsolation: "none",
  toolbar: false,
  search: false,
  watermark: false,
  fit: {
    mode: "width",
    resize: "always",
    padding: 0,
  },
  text: {
    toolbar: false,
    lineNumbers: false,
  },
  pdf: {
    toolbar: false,
    navigation: false,
    defaultNavigationVisible: false,
  },
  archive: {
    entryActions: {
      download: false,
    },
  },
} as const;

const FILE_PREVIEW_VIEWER_STYLE = {
  width: "100%",
  height: "100%",
  minHeight: 0,
  background: "transparent",
} as const;

// file-viewer 的 Markdown renderer 默认使用“文档卡片”布局；预览插件只
// 需要内容，不需要卡片、阴影和大块留白，因此在 renderer 之外统一收紧。
const FILE_PREVIEW_RENDERER_CSS = `
.file-preview-viewer .markdown-viewer {
  padding: 0 !important;
  background: transparent !important;
}
.file-preview-viewer .markdown-body {
  width: 100% !important;
  min-width: 0 !important;
  max-width: none !important;
  margin: 0 !important;
  padding: 18px 24px 28px !important;
  border: 0 !important;
  border-radius: 0 !important;
  box-shadow: none !important;
}
.file-preview-viewer .code-viewer {
  background: transparent !important;
}
.file-preview-viewer .code-toolbar {
  display: none !important;
}
.file-preview-viewer .code-area {
  padding: 16px 24px 24px !important;
}
`;

export function FilePreviewPlugin() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewResult | null>(null);
  const [fileBlob, setFileBlob] = useState<Blob | null>(null);
  const [fileName, setFileName] = useState<string>("");

  const choose = async () => {
    setError(null);
    const path = await pickPreviewFile().catch((e) => {
      setError(String(e));
      return null;
    });
    if (!path) return;
    setLoading(true);
    setPreview(null);
    setFileBlob(null);
    setFileName("");
    try {
      // 1. 获取文件元数据和文本提取。
      const result = await previewOpenFile(path);
      setPreview(result);
      setFileName(result.metadata.name);

      // 2. 读取文件二进制数据
      let blob: Blob;
      
      // 对于图片文件，优先使用 previewReadDataUrl（支持更多格式）
      if (result.metadata.kind === "image") {
        try {
          const dataUrl = await previewReadDataUrl(path);
          const response = await fetch(dataUrl);
          blob = await response.blob();
        } catch (imgError) {
          // 如果失败，降级到使用 asset 协议
          const assetUrl = convertFileSrc(path);
          const response = await fetch(assetUrl);
          blob = await response.blob();
        }
      } else {
        // 对于非图片文件，使用 Tauri asset 协议读取
        const assetUrl = convertFileSrc(path);
        const response = await fetch(assetUrl);
        blob = await response.blob();
      }
      
      setFileBlob(blob);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="w-full h-full flex flex-col bg-white">
      <div className="flex items-center justify-between border-b border-border-theme px-4 py-2 flex-shrink-0">
        <button
          type="button"
          onClick={choose}
          className="flex flex-shrink-0 whitespace-nowrap items-center text-[13px] text-text-base px-3 py-1.5 rounded-lg border border-border-theme hover:border-primary/50 hover:text-primary transition-colors"
        >
          <FontAwesomeIcon icon={["far", "folder-open"]} className="mr-2" />
          {t("plugins.filePreview.choose")}
        </button>
        {preview && (
          <div className="flex items-center min-w-0 ml-3">
            <div className="flex items-center min-w-0 text-[12px] text-text-secondary">
              <FontAwesomeIcon icon={kindIcon(preview.metadata.kind)} className="mr-2 flex-shrink-0" />
              <span className="truncate" title={preview.metadata.path}>
                {preview.metadata.name}
              </span>
              <span className="ml-2 flex-shrink-0">· {formatSize(preview.metadata.size_bytes)}</span>
              <span className="ml-2 flex-shrink-0 uppercase">{preview.metadata.ext || "?"}</span>
            </div>
          </div>
        )}
      </div>

      <div className="flex-1 min-h-0 overflow-hidden bg-white">
        {loading && (
          <div className="flex h-full items-center justify-center text-text-secondary text-[14px]">
            <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2" />
            {t("plugins.filePreview.loading")}
          </div>
        )}

        {!loading && error && (
          <div className="h-full p-4 text-[13px] text-red-500 whitespace-pre-wrap">{error}</div>
        )}

        {!loading && !error && !preview && (
          <div className="h-full flex flex-col items-center justify-center text-text-secondary">
            <FontAwesomeIcon icon={["far", "file-lines"]} className="text-3xl mb-3 opacity-60" />
            <div className="text-[13px]">{t("plugins.filePreview.empty")}</div>
          </div>
        )}

        {!loading && !error && preview && fileBlob && (
          <PreviewBody fileBlob={fileBlob} fileName={fileName} />
        )}
      </div>
    </div>
  );
}

export const filePreviewPluginDefinition: PluginDefinition = {
  type: "file_preview",
  icon: ["far", "file-lines"],
  titleKey: "file_preview",
  descKey: "filePreviewDesc",
  fallbackTitle: "File Preview",
  fallbackDesc: "Preview office documents",
  getTabTitle: ({ t }) =>
    t?.("chatView.tools.file_preview", { defaultValue: "File Preview" }) ||
    "File Preview",
  render: () => <FilePreviewPlugin />,
};

const PreviewBody = memo(function PreviewBody({
  fileBlob,
  fileName,
}: {
  fileBlob: Blob;
  fileName: string;
}) {
  const [FileViewer, setFileViewer] = useState<any>(null);
  const [viewerError, setViewerError] = useState<string | null>(null);

  useEffect(() => {
    // 动态导入 full 包，避免构建时缺少依赖报错并直接获得 208+ 格式矩阵。
    import("@file-viewer/react-full")
      .then((mod) => {
        setFileViewer(() => mod.default);
      })
      .catch((err) => {
        console.error("Failed to load @file-viewer/react-full:", err);
        setViewerError("文件预览组件加载失败，请确保已安装 @file-viewer/react-full");
      });
  }, []);

  if (viewerError) {
    return (
      <div className="text-[13px] text-red-500 whitespace-pre-wrap">
        {viewerError}
      </div>
    );
  }

  if (!FileViewer) {
    return (
      <div className="flex items-center justify-center h-full">
        <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2 text-text-secondary" />
        <span className="text-[13px] text-text-secondary">加载预览组件...</span>
      </div>
    );
  }

  return (
    <div className="w-full h-full min-h-0 overflow-hidden bg-white">
      <FileViewer
        className="file-preview-viewer block h-full w-full"
        file={fileBlob}
        name={fileName}
        style={FILE_PREVIEW_VIEWER_STYLE}
        options={FILE_PREVIEW_VIEWER_OPTIONS}
      />
      <style>{FILE_PREVIEW_RENDERER_CSS}</style>
    </div>
  );
});
