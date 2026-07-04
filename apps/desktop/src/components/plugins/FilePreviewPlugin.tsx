import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { PreviewResult } from "../../types";
import { pickPreviewFile, previewOpenFile, previewReadDataUrl, previewRenderPages, sendToChat } from "../../api";
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

/** Split CSV/TSV text into a row/column matrix for tabular rendering. */
function parseDelimited(text: string, sep: string): string[][] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.length > 0)
    .slice(0, 200)
    .map((line) => line.split(sep));
}

export function FilePreviewPlugin() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewResult | null>(null);
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [pdfPages, setPdfPages] = useState<string[] | null>(null);

  const choose = async () => {
    setError(null);
    const path = await pickPreviewFile().catch((e) => {
      setError(String(e));
      return null;
    });
    if (!path) return;
    setLoading(true);
    setPreview(null);
    setImageUrl(null);
    setPdfPages(null);
    try {
      const result = await previewOpenFile(path);
      setPreview(result);
      if (result.metadata.kind === "image") {
        const url = await previewReadDataUrl(path).catch(() => null);
        setImageUrl(url);
      } else if (result.metadata.kind === "pdf") {
        // Try Tier R page rendering; load each page PNG as a data URL.
        const r = await previewRenderPages(path).catch(() => null);
        if (r && r.rendered && r.pages.length > 0) {
          const urls = await Promise.all(
            r.pages.map((p) => previewReadDataUrl(p).catch(() => null))
          );
          setPdfPages(urls.filter((u): u is string => !!u));
        }
      }
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

        {!loading && !error && preview && <PreviewBody preview={preview} imageUrl={imageUrl} pdfPages={pdfPages} />}
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
  preview,
  imageUrl,
  pdfPages,
}: {
  preview: PreviewResult;
  imageUrl: string | null;
  pdfPages: string[] | null;
}) {
  const { t } = useTranslation();
  const { kind, ext } = preview.metadata;

  if (kind === "image") {
    if (!imageUrl) {
      return <div className="text-[13px] text-text-secondary">{t("plugins.filePreview.imageUnavailable")}</div>;
    }
    return (
      <div className="flex items-center justify-center">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={imageUrl} alt={preview.metadata.name} className="max-w-full max-h-full object-contain" />
      </div>
    );
  }

  if (kind === "xlsx") {
    const sheets = preview.sheets ?? [];
    if (sheets.length === 0) {
      return <div className="text-[13px] text-text-secondary">{t("plugins.filePreview.noSheets")}</div>;
    }
    return (
      <div className="space-y-6">
        {sheets.map((sheet) => (
          <div key={sheet.name}>
            <div className="text-[13px] font-semibold text-text-base mb-2 flex items-center">
              <FontAwesomeIcon icon={["fas", "table"]} className="mr-2 text-text-secondary" />
              {sheet.name}
              {sheet.truncated && (
                <span className="ml-2 text-[11px] text-text-secondary font-normal">
                  {t("plugins.filePreview.truncatedRows")}
                </span>
              )}
            </div>
            <div className="overflow-auto border border-border-theme rounded-lg">
              <table className="text-[12px] border-collapse">
                <tbody>
                  {sheet.rows.map((row, ri) => (
                    <tr key={ri} className={ri === 0 ? "bg-gray-50 font-medium" : ""}>
                      {row.map((cell, ci) => (
                        <td key={ci} className="border border-border-theme px-2 py-1 whitespace-nowrap">
                          {cell}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (kind === "csv") {
    const sep = ext === "tsv" ? "\t" : ",";
    const rows = parseDelimited(preview.text ?? "", sep);
    return (
      <div className="overflow-auto border border-border-theme rounded-lg">
        <table className="text-[12px] border-collapse">
          <tbody>
            {rows.map((row, ri) => (
              <tr key={ri} className={ri === 0 ? "bg-gray-50 font-medium" : ""}>
                {row.map((cell, ci) => (
                  <td key={ci} className="border border-border-theme px-2 py-1 whitespace-nowrap">
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }

  // text / docx / pptx / pdf-text / unknown-with-message
  if (kind === "pdf" && pdfPages && pdfPages.length > 0) {
    return (
      <div className="space-y-3">
        {pdfPages.map((src, i) => (
          // eslint-disable-next-line @next/next/no-img-element
          <img key={i} src={src} alt={`page ${i + 1}`} className="max-w-full border border-border-theme rounded" />
        ))}
      </div>
    );
  }

  if (preview.text != null) {
    return (
      <>
        {preview.truncated && (
          <div className="text-[11px] text-amber-600 mb-2">{t("plugins.filePreview.truncated")}</div>
        )}
        <pre className="text-[13px] text-text-base whitespace-pre-wrap font-mono leading-relaxed">
          {preview.text}
        </pre>
      </>
    );
  }

  return (
    <div className="text-[13px] text-text-secondary">
      {preview.message ?? t("plugins.filePreview.unsupported")}
    </div>
  );
}
