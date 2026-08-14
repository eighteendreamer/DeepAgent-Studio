import { useState, useEffect } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { PreviewResult } from "../../types";
import { pickPreviewFile, previewOpenFile, previewReadDataUrl, sendToChat } from "../../api";
import type { PluginDefinition } from "./pluginTypes";

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
      // 1. 获取文件元数据和文本提取（用于"发送到聊天"功能）
      const result = await previewOpenFile(path);
      setPreview(result);
      setFileName(result.metadata.name);

      // 2. 使用后端 API 读取文件为 base64 data URL，然后转换为 Blob
      const dataUrl = await previewReadDataUrl(path);
      
      // 将 data URL 转换为 Blob
      const response = await fetch(dataUrl);
      const blob = await response.blob();
      setFileBlob(blob);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const sendToChatContext = () => {
    if (!preview) return;
    const { name, kind, path } = preview.metadata;
    let summary = "";
    if (preview.text) {
      summary = preview.text.slice(0, 2000);
    } else if (preview.sheets && preview.sheets.length > 0) {
      summary = preview.sheets
        .map((s) => `# ${s.name}\n` + s.rows.slice(0, 20).map((r) => r.join("\t")).join("\n"))
        .join("\n\n")
        .slice(0, 2000);
    }
    sendToChat(
      `<office-context>\n当前预览文件: ${name}\n文件类型: ${kind}\n路径: ${path}\n提取内容摘要:\n${summary}\n</office-context>\n\n这是我正在预览的文件，请帮我摘要、修改、转换或生成新版本。`
    );
  };

  return (
    <div className="w-full h-full flex flex-col bg-white">
      {/* Toolbar */}
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
            <button
              type="button"
              onClick={sendToChatContext}
              className="ml-3 flex-shrink-0 text-[12px] text-primary hover:underline whitespace-nowrap"
            >
              {t("plugins.filePreview.sendToChat")}
            </button>
          </div>
        )}
      </div>

      {/* Body */}
      <div className="flex-1 overflow-auto p-4">
        {loading && (
          <div className="flex items-center text-text-secondary text-[14px]">
            <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2" />
            {t("plugins.filePreview.loading")}
          </div>
        )}

        {!loading && error && (
          <div className="text-[13px] text-red-500 whitespace-pre-wrap">{error}</div>
        )}

        {!loading && !error && !preview && (
          <div className="h-full flex flex-col items-center justify-center text-text-secondary">
            <FontAwesomeIcon icon={["far", "file-lines"]} className="text-3xl mb-3 opacity-60" />
            <div className="text-[13px]">{t("plugins.filePreview.empty")}</div>
          </div>
        )}

        {!loading && !error && preview && fileBlob && (
          <PreviewBody preview={preview} fileBlob={fileBlob} fileName={fileName} />
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

function PreviewBody({
  fileBlob,
  fileName,
}: {
  preview: PreviewResult;
  fileBlob: Blob;
  fileName: string;
}) {
  const [FileViewer, setFileViewer] = useState<any>(null);
  const [viewerError, setViewerError] = useState<string | null>(null);

  useEffect(() => {
    // 动态导入 @file-viewer/react，避免构建时缺少依赖报错
    import("@file-viewer/react")
      .then((mod) => {
        setFileViewer(() => mod.default);
      })
      .catch((err) => {
        console.error("Failed to load @file-viewer/react:", err);
        setViewerError("文件预览组件加载失败，请确保已安装 @file-viewer/react");
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
    <div className="w-full h-full">
      <FileViewer
        file={fileBlob}
        name={fileName}
        onEvent={(event: any) => {
          console.log("FileViewer event:", event.type, event.payload);
        }}
        options={{
          theme: "light",
          rendererMode: "replace",
          styleIsolation: "shadow",
          toolbar: {
            position: "bottom-right",
          },
          watermark: {
            text: "DeepAgent Studio",
            opacity: 0.08,
          },
        }}
      />
    </div>
  );
}
