import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import {
  listProjectFiles,
  openProjectInFileManager,
  previewOpenFile,
  previewReadDataUrl,
  previewRenderPages,
} from "../../api";
import type { PreviewResult, ProjectFileEntry } from "../../types";
import { MarkdownText } from "../MarkdownText";
import { message as toast } from "../message";

interface FilesPluginProps {
  projectPath?: string | null;
}

const ROOT_KEY = "__root__";
const FILE_TREE_COLLAPSED_KEY = "deepagent:files-plugin-tree-collapsed";
const FILE_ENHANCED_VIEW_KEY = "deepagent:files-plugin-enhanced-view";
const PREVIEWABLE_EXTS = new Set([
  "md",
  "markdown",
  "txt",
  "json",
  "ts",
  "tsx",
  "js",
  "jsx",
  "mjs",
  "cjs",
  "css",
  "scss",
  "sass",
  "less",
  "html",
  "htm",
  "vue",
  "svelte",
  "java",
  "kt",
  "kts",
  "gradle",
  "groovy",
  "rs",
  "go",
  "py",
  "rb",
  "php",
  "c",
  "cc",
  "cpp",
  "cxx",
  "h",
  "hpp",
  "hxx",
  "cs",
  "swift",
  "sql",
  "sh",
  "bash",
  "zsh",
  "fish",
  "ps1",
  "bat",
  "cmd",
  "yaml",
  "yml",
  "toml",
  "xml",
  "conf",
  "cfg",
  "ini",
  "properties",
  "env",
  "lock",
  "csv",
  "tsv",
  "log",
  "pdf",
  "docx",
  "xlsx",
  "pptx",
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
  "svg",
]);
const PREFERRED_FILE_NAMES = [
  "readme.md",
  "readme",
  "package.json",
  "cargo.toml",
  "tsconfig.json",
  "pyproject.toml",
];

function getPathLabel(path: string | null | undefined): string {
  if (!path) return "";
  const normalized = path.replace(/[\\/]+$/, "");
  const parts = normalized.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? normalized;
}

function formatSize(bytes: number | null | undefined): string {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) return "--";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function parseDelimited(text: string, sep: string): string[][] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.length > 0)
    .slice(0, 200)
    .map((line) => line.split(sep));
}

function splitLines(text: string): string[] {
  const lines = text.split(/\r?\n/);
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function getStoredBool(key: string, fallback: boolean): boolean {
  if (typeof window === "undefined") return fallback;
  const value = window.localStorage.getItem(key);
  if (value == null) return fallback;
  return value === "true";
}

function fileIcon(entry: ProjectFileEntry): IconProp {
  if (entry.is_dir) return ["far", "folder"];
  switch (entry.ext) {
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "bmp":
    case "svg":
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
    case "tsv":
      return ["fas", "table"];
    case "md":
    case "markdown":
      return ["far", "file-lines"];
    default:
      return ["far", "file-lines"];
  }
}

function pickInitialFile(entries: ProjectFileEntry[]): ProjectFileEntry | null {
  const files = entries.filter((entry) => !entry.is_dir);
  if (files.length === 0) return null;
  const sorted = [...files].sort((a, b) => {
    const aName = a.name.toLowerCase();
    const bName = b.name.toLowerCase();
    const aPreferred = PREFERRED_FILE_NAMES.indexOf(aName);
    const bPreferred = PREFERRED_FILE_NAMES.indexOf(bName);
    if (aPreferred !== bPreferred) {
      if (aPreferred === -1) return 1;
      if (bPreferred === -1) return -1;
      return aPreferred - bPreferred;
    }
    const aPreviewable = PREVIEWABLE_EXTS.has(a.ext);
    const bPreviewable = PREVIEWABLE_EXTS.has(b.ext);
    if (aPreviewable !== bPreviewable) return aPreviewable ? -1 : 1;
    return aName.localeCompare(bName);
  });
  return sorted[0] ?? null;
}

function matchesFilter(
  entry: ProjectFileEntry,
  query: string,
  directoryMap: Record<string, ProjectFileEntry[]>,
): boolean {
  if (!query) return true;
  if (entry.name.toLowerCase().includes(query)) return true;
  if (!entry.is_dir) return false;
  const children = directoryMap[entry.path] ?? [];
  return children.some((child) => matchesFilter(child, query, directoryMap));
}

function PlainTextPreview({
  text,
  showLineNumbers,
}: {
  text: string;
  showLineNumbers: boolean;
}) {
  const lines = splitLines(text);

  if (!showLineNumbers) {
    return (
      <div className="px-8 py-6">
        <pre className="whitespace-pre-wrap break-words font-mono text-[13px] leading-7 text-text-base">
          {text}
        </pre>
      </div>
    );
  }

  return (
    <div className="grid min-h-full grid-cols-[56px_minmax(0,1fr)] bg-white font-mono text-[13px] leading-7">
      <div className="select-none border-r border-border-theme bg-[#fbfbfc] px-3 py-5 text-right text-[12px] text-text-secondary">
        {lines.map((_, index) => (
          <div key={index}>{index + 1}</div>
        ))}
      </div>
      <div className="overflow-auto px-5 py-5 text-text-base">
        <pre className="whitespace-pre-wrap break-words">{text}</pre>
      </div>
    </div>
  );
}

function PreviewBody({
  preview,
  imageUrl,
  pdfPages,
  enhancedViewEnabled,
}: {
  preview: PreviewResult;
  imageUrl: string | null;
  pdfPages: string[] | null;
  enhancedViewEnabled: boolean;
}) {
  const { t } = useTranslation();
  const { kind, ext } = preview.metadata;

  if (kind === "image") {
    if (!imageUrl) {
      return <div className="px-8 py-6 text-[13px] text-text-secondary">{t("plugins.filePreview.imageUnavailable")}</div>;
    }
    return (
      <div className="flex h-full items-center justify-center px-8 py-8">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={imageUrl} alt={preview.metadata.name} className="max-h-full max-w-full object-contain" />
      </div>
    );
  }

  if (kind === "xlsx") {
    const sheets = preview.sheets ?? [];
    if (sheets.length === 0) {
      return <div className="px-8 py-6 text-[13px] text-text-secondary">{t("plugins.filePreview.noSheets")}</div>;
    }
    return (
      <div className="space-y-6 px-8 py-6">
        {sheets.map((sheet) => (
          <div key={sheet.name}>
            <div className="mb-2 flex items-center text-[13px] font-semibold text-text-base">
              <FontAwesomeIcon icon={["fas", "table"]} className="mr-2 text-text-secondary" />
              {sheet.name}
              {sheet.truncated ? (
                <span className="ml-2 text-[11px] font-normal text-text-secondary">
                  {t("plugins.filePreview.truncatedRows")}
                </span>
              ) : null}
            </div>
            <div className="overflow-auto rounded-lg border border-border-theme">
              <table className="border-collapse text-[12px]">
                <tbody>
                  {sheet.rows.map((row, rowIndex) => (
                    <tr key={`${sheet.name}-${rowIndex}`} className={rowIndex === 0 ? "bg-gray-50 font-medium" : ""}>
                      {row.map((cell, cellIndex) => (
                        <td
                          key={`${sheet.name}-${rowIndex}-${cellIndex}`}
                          className="whitespace-nowrap border border-border-theme px-2 py-1"
                        >
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
    const rows = parseDelimited(preview.text ?? "", ext === "tsv" ? "\t" : ",");
    return (
      <div className="overflow-auto px-8 py-6">
        <table className="border-collapse text-[12px]">
          <tbody>
            {rows.map((row, rowIndex) => (
              <tr key={rowIndex} className={rowIndex === 0 ? "bg-gray-50 font-medium" : ""}>
                {row.map((cell, cellIndex) => (
                  <td key={`${rowIndex}-${cellIndex}`} className="whitespace-nowrap border border-border-theme px-2 py-1">
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

  if (kind === "pdf" && pdfPages && pdfPages.length > 0) {
    return (
      <div className="space-y-4 px-8 py-6">
        {pdfPages.map((src, index) => (
          // eslint-disable-next-line @next/next/no-img-element
          <img key={index} src={src} alt={`page ${index + 1}`} className="max-w-full rounded border border-border-theme" />
        ))}
      </div>
    );
  }

  if (preview.text != null) {
    return (
      <>
        {preview.truncated ? (
          <div className="px-8 pt-4 text-[11px] text-amber-600">{t("plugins.filePreview.truncated")}</div>
        ) : null}
        {ext === "md" || ext === "markdown" ? (
          enhancedViewEnabled ? (
            <div className="px-8 py-6">
              <MarkdownText text={preview.text} className="max-w-none text-[14px] leading-7" />
            </div>
          ) : (
            <PlainTextPreview text={preview.text} showLineNumbers={false} />
          )
        ) : (
          <PlainTextPreview text={preview.text} showLineNumbers={enhancedViewEnabled} />
        )}
      </>
    );
  }

  return (
    <div className="px-8 py-6 text-[13px] text-text-secondary">
      {preview.message ?? t("plugins.filePreview.unsupported")}
    </div>
  );
}

export function FilesPlugin({ projectPath = null }: FilesPluginProps) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState("");
  const [rootPath, setRootPath] = useState<string | null>(null);
  const [directoryMap, setDirectoryMap] = useState<Record<string, ProjectFileEntry[]>>({});
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(() => new Set());
  const [loadingDirs, setLoadingDirs] = useState<Record<string, boolean>>({});
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewResult | null>(null);
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [pdfPages, setPdfPages] = useState<string[] | null>(null);
  const [treeCollapsed, setTreeCollapsed] = useState<boolean>(() =>
    getStoredBool(FILE_TREE_COLLAPSED_KEY, false),
  );
  const [enhancedViewEnabled, setEnhancedViewEnabled] = useState<boolean>(() =>
    getStoredBool(FILE_ENHANCED_VIEW_KEY, true),
  );
  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false);
  const [isOpenMenuOpen, setIsOpenMenuOpen] = useState(false);
  const browseRunRef = useRef(0);
  const previewRunRef = useRef(0);
  const moreMenuRef = useRef<HTMLDivElement>(null);
  const openMenuRef = useRef<HTMLDivElement>(null);

  const rootEntries = directoryMap[ROOT_KEY] ?? [];
  const filterQuery = filter.trim().toLowerCase();

  useEffect(() => {
    window.localStorage.setItem(FILE_TREE_COLLAPSED_KEY, String(treeCollapsed));
  }, [treeCollapsed]);

  useEffect(() => {
    window.localStorage.setItem(FILE_ENHANCED_VIEW_KEY, String(enhancedViewEnabled));
  }, [enhancedViewEnabled]);

  useEffect(() => {
    if (!isMoreMenuOpen && !isOpenMenuOpen) return;
    const onMouseDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (moreMenuRef.current?.contains(target)) return;
      if (openMenuRef.current?.contains(target)) return;
      setIsMoreMenuOpen(false);
      setIsOpenMenuOpen(false);
    };
    document.addEventListener("mousedown", onMouseDown);
    return () => document.removeEventListener("mousedown", onMouseDown);
  }, [isMoreMenuOpen, isOpenMenuOpen]);

  const entryIndex = useMemo(() => {
    const map = new Map<string, ProjectFileEntry>();
    Object.values(directoryMap).forEach((entries) => {
      entries.forEach((entry) => {
        map.set(entry.path, entry);
      });
    });
    return map;
  }, [directoryMap]);

  const selectedEntry = selectedPath ? entryIndex.get(selectedPath) ?? null : null;
  const selectedIsDirectory = (!!rootPath && selectedPath === rootPath) || !!selectedEntry?.is_dir;
  const selectedDirectoryEntries = useMemo(() => {
    if (!rootPath || !selectedPath) return [];
    if (selectedPath === rootPath) return rootEntries;
    if (selectedEntry?.is_dir) return directoryMap[selectedEntry.path] ?? [];
    return [];
  }, [directoryMap, rootEntries, rootPath, selectedEntry, selectedPath]);

  const projectLabel = useMemo(() => {
    return getPathLabel(rootPath ?? projectPath ?? "") || t("chatView.tools.files", { defaultValue: "文件" });
  }, [projectPath, rootPath, t]);

  const selectedRelPath = selectedEntry?.rel_path ?? "";
  const selectedOpenPath = selectedPath ?? rootPath;
  const breadcrumbSegments = useMemo(() => {
    const segments = [projectLabel];
    if (!selectedRelPath) return segments;
    return segments.concat(selectedRelPath.split(/[\\/]/).filter(Boolean));
  }, [projectLabel, selectedRelPath]);

  const directoryStats = useMemo(() => {
    const dirs = selectedDirectoryEntries.filter((entry) => entry.is_dir).length;
    const files = selectedDirectoryEntries.filter((entry) => !entry.is_dir).length;
    return { dirs, files };
  }, [selectedDirectoryEntries]);

  const loadDirectory = useCallback(
    async (targetPath: string) => {
      if (directoryMap[targetPath]) return;
      setLoadingDirs((prev) => ({ ...prev, [targetPath]: true }));
      try {
        const result = await listProjectFiles(projectPath, targetPath);
        setRootPath(result.root_path);
        setTreeError(null);
        setDirectoryMap((prev) => ({ ...prev, [targetPath]: result.entries }));
      } catch (error) {
        setTreeError(String(error));
      } finally {
        setLoadingDirs((prev) => ({ ...prev, [targetPath]: false }));
      }
    },
    [directoryMap, projectPath],
  );

  useEffect(() => {
    let disposed = false;
    const runId = browseRunRef.current + 1;
    browseRunRef.current = runId;

    setFilter("");
    setRootPath(null);
    setDirectoryMap({});
    setExpandedDirs(new Set());
    setSelectedPath(null);
    setTreeError(null);
    setPreviewError(null);
    setPreview(null);
    setImageUrl(null);
    setPdfPages(null);
    setTreeLoading(true);

    void (async () => {
      try {
        const result = await listProjectFiles(projectPath, null);
        if (disposed || browseRunRef.current !== runId) return;
        const initialFile = pickInitialFile(result.entries);
        setRootPath(result.root_path);
        setDirectoryMap({ [ROOT_KEY]: result.entries });
        setExpandedDirs(new Set());
        setSelectedPath(initialFile?.path ?? result.root_path);
      } catch (error) {
        if (disposed || browseRunRef.current !== runId) return;
        setTreeError(String(error));
      } finally {
        if (!disposed && browseRunRef.current === runId) {
          setTreeLoading(false);
        }
      }
    })();

    return () => {
      disposed = true;
    };
  }, [projectPath]);

  useEffect(() => {
    if (!rootPath || !selectedPath || selectedIsDirectory) {
      setPreviewLoading(false);
      setPreviewError(null);
      setPreview(null);
      setImageUrl(null);
      setPdfPages(null);
      return;
    }

    let disposed = false;
    const runId = previewRunRef.current + 1;
    previewRunRef.current = runId;

    setPreviewLoading(true);
    setPreviewError(null);
    setPreview(null);
    setImageUrl(null);
    setPdfPages(null);

    void (async () => {
      try {
        const result = await previewOpenFile(selectedPath);
        if (disposed || previewRunRef.current !== runId) return;
        setPreview(result);

        if (result.metadata.kind === "image") {
          const url = await previewReadDataUrl(selectedPath).catch(() => null);
          if (!disposed && previewRunRef.current === runId) {
            setImageUrl(url);
          }
        } else if (result.metadata.kind === "pdf") {
          const rendered = await previewRenderPages(selectedPath).catch(() => null);
          if (rendered && rendered.rendered && rendered.pages.length > 0) {
            const urls = await Promise.all(
              rendered.pages.map((pagePath) => previewReadDataUrl(pagePath).catch(() => null)),
            );
            if (!disposed && previewRunRef.current === runId) {
              setPdfPages(urls.filter((url): url is string => !!url));
            }
          }
        }
      } catch (error) {
        if (!disposed && previewRunRef.current === runId) {
          setPreviewError(String(error));
        }
      } finally {
        if (!disposed && previewRunRef.current === runId) {
          setPreviewLoading(false);
        }
      }
    })();

    return () => {
      disposed = true;
    };
  }, [rootPath, selectedIsDirectory, selectedPath]);

  const toggleDirectory = async (entry: ProjectFileEntry) => {
    setSelectedPath(entry.path);
    const isExpanded = expandedDirs.has(entry.path);
    setExpandedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(entry.path)) {
        next.delete(entry.path);
      } else {
        next.add(entry.path);
      }
      return next;
    });
    if (!isExpanded) {
      await loadDirectory(entry.path);
    }
  };

  const copyToClipboard = async (text: string, successMessage: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(successMessage);
    } catch {
      toast.error("复制失败");
    }
  };

  const handleCopyCurrentPath = async () => {
    if (!selectedOpenPath) return;
    await copyToClipboard(selectedOpenPath, "路径已复制");
    setIsMoreMenuOpen(false);
  };

  const handleCopyFileContent = async () => {
    if (!preview?.text) {
      toast.info("当前文件没有可复制的文本内容");
      return;
    }
    await copyToClipboard(preview.text, "文件内容已复制");
    setIsMoreMenuOpen(false);
  };

  const handleOpenLocation = async (path: string | null) => {
    if (!path) return;
    try {
      await openProjectInFileManager(path);
      setIsOpenMenuOpen(false);
    } catch (error) {
      toast.error(String(error));
    }
  };

  const renderEntries = (entries: ProjectFileEntry[], depth = 0): JSX.Element[] =>
    entries.flatMap((entry) => {
      if (!matchesFilter(entry, filterQuery, directoryMap)) return [];
      const isExpanded = expandedDirs.has(entry.path);
      const isSelected = selectedPath === entry.path;
      const isLoadingChildren = !!loadingDirs[entry.path];
      const children = directoryMap[entry.path] ?? [];

      return [
        <div key={entry.path}>
          <div
            className={`group flex items-center rounded-xl px-2.5 py-1.5 text-[13px] transition-colors ${
              isSelected
                ? "bg-[#f3f4f6] text-text-base shadow-[inset_0_0_0_1px_rgba(15,23,42,0.05)]"
                : "text-text-base hover:bg-gray-50"
            }`}
            style={{ paddingLeft: `${10 + depth * 18}px` }}
            onClick={() => {
              if (entry.is_dir) {
                void toggleDirectory(entry);
              } else {
                setSelectedPath(entry.path);
              }
            }}
          >
            <span className="mr-1 flex h-4 w-4 items-center justify-center text-[10px] text-text-secondary">
              {entry.is_dir ? (
                <FontAwesomeIcon icon={["fas", isExpanded ? "chevron-down" : "chevron-right"]} />
              ) : null}
            </span>
            <FontAwesomeIcon icon={fileIcon(entry)} className="mr-2 w-4 text-text-secondary" />
            <span className="min-w-0 flex-1 truncate">{entry.name}</span>
            {entry.is_dir && isLoadingChildren ? (
              <FontAwesomeIcon icon={["fas", "circle-notch"]} className="ml-2 animate-spin text-[11px] text-text-secondary" />
            ) : null}
          </div>
          {entry.is_dir && (isExpanded || !!filterQuery) && children.length > 0 ? (
            <div>{renderEntries(children, depth + 1)}</div>
          ) : null}
        </div>,
      ];
    });

  return (
    <div className="flex h-full w-full flex-col bg-white">
      <div className="flex h-[40px] items-center justify-between border-b border-border-theme px-4">
        <div className="flex min-w-0 items-center text-[13px] text-text-secondary">
          {breadcrumbSegments.map((segment, index) => (
            <div key={`${segment}-${index}`} className="flex min-w-0 items-center">
              {index > 0 ? (
                <FontAwesomeIcon icon={["fas", "chevron-right"]} className="mx-2 text-[10px] text-[#a0a7b4]" />
              ) : null}
              <span
                className={`truncate ${index === breadcrumbSegments.length - 1 ? "font-semibold text-text-base" : ""}`}
                title={segment}
              >
                {segment}
              </span>
            </div>
          ))}
        </div>

        <div className="ml-4 flex flex-shrink-0 items-center gap-2">
          <div ref={moreMenuRef} className="relative">
            <button
              type="button"
              onClick={() => {
                setIsMoreMenuOpen((prev) => !prev);
                setIsOpenMenuOpen(false);
              }}
              className="flex h-8 w-8 items-center justify-center rounded-lg text-text-secondary transition-colors hover:bg-[#f4f5f7] hover:text-text-base"
            >
              <FontAwesomeIcon icon={["fas", "ellipsis"]} className="text-[12px]" />
            </button>
            {isMoreMenuOpen ? (
              <div className="absolute right-0 top-full z-20 mt-2 w-[220px] overflow-hidden rounded-2xl border border-border-theme bg-white p-1.5 shadow-[0_18px_36px_rgba(15,23,42,0.12)]">
                <button
                  type="button"
                  onClick={() => void handleCopyCurrentPath()}
                  className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-[14px] text-text-base transition-colors hover:bg-[#f7f8fa]"
                >
                  <FontAwesomeIcon icon={["far", "copy"]} className="text-[13px] text-text-secondary" />
                  <span>复制路径</span>
                </button>
                <button
                  type="button"
                  onClick={() => void handleCopyFileContent()}
                  className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-[14px] text-text-base transition-colors hover:bg-[#f7f8fa]"
                >
                  <FontAwesomeIcon icon={["far", "file-lines"]} className="text-[13px] text-text-secondary" />
                  <span>复制文件内容</span>
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setEnhancedViewEnabled((prev) => !prev);
                    setIsMoreMenuOpen(false);
                  }}
                  className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-[14px] text-text-base transition-colors hover:bg-[#f7f8fa]"
                >
                  <FontAwesomeIcon icon={["fas", "code"]} className="text-[13px] text-text-secondary" />
                  <span>{enhancedViewEnabled ? "禁用增强视图" : "启用增强视图"}</span>
                </button>
              </div>
            ) : null}
          </div>

          <div ref={openMenuRef} className="relative">
            <button
              type="button"
              onClick={() => {
                setIsOpenMenuOpen((prev) => !prev);
                setIsMoreMenuOpen(false);
              }}
              className="inline-flex h-8 items-center gap-2 rounded-lg border border-border-theme px-3 text-[13px] font-medium text-text-base transition-colors hover:bg-[#f7f8fa]"
            >
              <FontAwesomeIcon icon={["far", "folder-open"]} className="text-[12px] text-text-secondary" />
              <span>打开</span>
              <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
            </button>
            {isOpenMenuOpen ? (
              <div className="absolute right-0 top-full z-20 mt-2 w-[220px] overflow-hidden rounded-2xl border border-border-theme bg-white p-1.5 shadow-[0_18px_36px_rgba(15,23,42,0.12)]">
                <button
                  type="button"
                  onClick={() => void handleOpenLocation(selectedOpenPath)}
                  className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-[14px] text-text-base transition-colors hover:bg-[#f7f8fa]"
                >
                  <FontAwesomeIcon icon={["far", "folder-open"]} className="text-[13px] text-text-secondary" />
                  <span>打开当前文件位置</span>
                </button>
                <button
                  type="button"
                  onClick={() => void handleOpenLocation(rootPath)}
                  className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-[14px] text-text-base transition-colors hover:bg-[#f7f8fa]"
                >
                  <FontAwesomeIcon icon={["far", "folder"]} className="text-[13px] text-text-secondary" />
                  <span>打开项目根目录</span>
                </button>
              </div>
            ) : null}
          </div>

          <button
            type="button"
            onClick={() => setTreeCollapsed((prev) => !prev)}
            className="flex h-8 w-8 items-center justify-center rounded-lg text-text-secondary transition-colors hover:bg-[#f4f5f7] hover:text-text-base"
            title={treeCollapsed ? "展开文件树" : "收起文件树"}
          >
            <FontAwesomeIcon
              icon={["fas", treeCollapsed ? "angles-left" : "angles-right"]}
              className="text-[12px]"
            />
          </button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 overflow-hidden bg-white">
        <div className="min-w-0 flex-1 overflow-hidden">
          {treeLoading ? (
            <div className="flex h-full items-center justify-center px-6 text-[14px] text-text-secondary">
              <FontAwesomeIcon icon={["fas", "circle-notch"]} className="mr-2 animate-spin" />
              {t("plugins.filePreview.loading", { defaultValue: "正在加载..." })}
            </div>
          ) : treeError ? (
            <div className="whitespace-pre-wrap px-6 py-6 text-[13px] text-red-500">{treeError}</div>
          ) : selectedIsDirectory ? (
            <div className="h-full overflow-y-auto px-8 py-6">
              <h1 className="mb-2 text-[26px] font-semibold text-text-base">{selectedEntry?.name ?? projectLabel}</h1>
              <div className="mb-5 flex items-center gap-4 text-[12px] text-text-secondary">
                <span>{directoryStats.dirs} 个目录</span>
                <span>{directoryStats.files} 个文件</span>
              </div>

              {selectedDirectoryEntries.length === 0 ? (
                <div className="text-[13px] text-text-secondary">
                  {t("plugins.files.emptyDirectory", { defaultValue: "这个目录是空的。" })}
                </div>
              ) : (
                <div className="overflow-hidden rounded-xl border border-border-theme">
                  <table className="w-full border-collapse text-left text-[13px]">
                    <thead className="bg-[#fbfbfc] text-text-secondary">
                      <tr>
                        <th className="px-4 py-3 font-medium">名称</th>
                        <th className="px-4 py-3 font-medium">类型</th>
                        <th className="px-4 py-3 font-medium">大小</th>
                      </tr>
                    </thead>
                    <tbody>
                      {selectedDirectoryEntries.map((entry) => (
                        <tr
                          key={entry.path}
                          className="border-t border-border-theme transition-colors hover:bg-[#fafafa]"
                          onClick={() => {
                            if (entry.is_dir) {
                              void toggleDirectory(entry);
                            } else {
                              setSelectedPath(entry.path);
                            }
                          }}
                        >
                          <td className="px-4 py-3">
                            <div className="flex items-center">
                              <FontAwesomeIcon icon={fileIcon(entry)} className="mr-2 w-4 text-text-secondary" />
                              <span className="truncate">{entry.name}</span>
                            </div>
                          </td>
                          <td className="px-4 py-3 text-text-secondary">
                            {entry.is_dir ? "目录" : (entry.ext || "文件").toUpperCase()}
                          </td>
                          <td className="px-4 py-3 text-text-secondary">
                            {entry.is_dir ? "--" : formatSize(entry.size_bytes)}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          ) : previewLoading ? (
            <div className="flex h-full items-center justify-center px-6 text-[14px] text-text-secondary">
              <FontAwesomeIcon icon={["fas", "circle-notch"]} className="mr-2 animate-spin" />
              {t("plugins.filePreview.loading")}
            </div>
          ) : previewError ? (
            <div className="whitespace-pre-wrap px-6 py-6 text-[13px] text-red-500">{previewError}</div>
          ) : preview ? (
            <div className="h-full overflow-y-auto bg-white">
              <PreviewBody
                preview={preview}
                imageUrl={imageUrl}
                pdfPages={pdfPages}
                enhancedViewEnabled={enhancedViewEnabled}
              />
            </div>
          ) : (
            <div className="flex h-full items-center justify-center text-[13px] text-text-secondary">
              {t("plugins.filePreview.empty")}
            </div>
          )}
        </div>

        {!treeCollapsed ? (
          <div className="flex w-[340px] flex-shrink-0 flex-col border-l border-border-theme bg-white">
            <div className="border-b border-border-theme px-4 py-4">
              <div className="mb-3 truncate text-[14px] font-semibold text-text-base" title={rootPath ?? undefined}>
                {projectLabel}
              </div>
              <div className="relative">
                <FontAwesomeIcon
                  icon={["fas", "magnifying-glass"]}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-[13px] text-text-secondary"
                />
                <input
                  type="text"
                  value={filter}
                  onChange={(event) => setFilter(event.target.value)}
                  placeholder={t("plugins.files.filterPlaceholder")}
                  className="h-10 w-full rounded-2xl border border-border-theme bg-[#fbfbfc] px-10 pr-3 text-[13px] outline-none transition-colors focus:border-primary/30"
                />
              </div>
            </div>
            <div className="flex-1 overflow-y-auto px-3 py-3 text-[13px] text-text-base">
              {rootEntries.length === 0 && !treeLoading ? (
                <div className="px-2 py-4 text-[12px] text-text-secondary">
                  {t("plugins.files.emptyDirectory", { defaultValue: "这个目录是空的。" })}
                </div>
              ) : (
                renderEntries(rootEntries)
              )}
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
