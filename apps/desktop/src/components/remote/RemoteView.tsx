import { useEffect, useState } from "react";
import { FileText, Loader2, Server, Terminal, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { TerminalPlugin } from "../plugins/TerminalPlugin";
import { SyntaxHighlightedCode } from "../SyntaxHighlightedCode";
import { sshReadFile, type SshConnection } from "../../api";

interface RemoteViewProps {
  connection: SshConnection | null;
  openFile: { path: string; name: string; token: number } | null;
}

interface FileTab {
  path: string;
  name: string;
}

type FileTabState =
  | { status: "loading" }
  | { status: "loaded"; content: string; truncated: boolean }
  | { status: "error"; message: string };

const TERMINAL_TAB_KEY = "terminal";
const MAX_FILE_TABS = 8;

const EXT_LANGUAGE: Record<string, string> = {
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  css: "css",
  go: "go",
  java: "java",
  js: "javascript",
  mjs: "javascript",
  json: "json",
  jsx: "jsx",
  html: "markup",
  htm: "markup",
  xml: "markup",
  md: "markup",
  py: "python",
  rs: "rust",
  sql: "sql",
  tsx: "tsx",
  ts: "typescript",
};

const languageForFile = (name: string) => {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  return EXT_LANGUAGE[ext] ?? "";
};

export function RemoteView({ connection, openFile }: RemoteViewProps) {
  const { t } = useTranslation();
  const [fileTabs, setFileTabs] = useState<FileTab[]>([]);
  const [activeKey, setActiveKey] = useState<string>(TERMINAL_TAB_KEY);
  const [fileStates, setFileStates] = useState<Record<string, FileTabState>>({});

  const connectionId = connection?.id ?? null;
  const connected = connection?.status === "connected";

  useEffect(() => {
    setFileTabs([]);
    setFileStates({});
    setActiveKey(TERMINAL_TAB_KEY);
  }, [connectionId]);

  useEffect(() => {
    if (!connected || !openFile) return;
    setFileTabs((prev) => {
      if (prev.some((tab) => tab.path === openFile.path)) return prev;
      const next = [...prev, { path: openFile.path, name: openFile.name }];
      return next.length > MAX_FILE_TABS ? next.slice(next.length - MAX_FILE_TABS) : next;
    });
    setActiveKey(openFile.path);
    setFileStates((prev) =>
      prev[openFile.path] ? prev : { ...prev, [openFile.path]: { status: "loading" } },
    );
  }, [openFile, connected]);

  useEffect(() => {
    if (!connectionId || !connected) return;
    const loadingPath = Object.keys(fileStates).find(
      (path) => fileStates[path].status === "loading",
    );
    if (!loadingPath) return;
    let cancelled = false;
    sshReadFile(connectionId, loadingPath)
      .then((result) => {
        if (!cancelled) {
          setFileStates((prev) => ({
            ...prev,
            [loadingPath]: {
              status: "loaded",
              content: result.content,
              truncated: result.truncated,
            },
          }));
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setFileStates((prev) => ({
            ...prev,
            [loadingPath]: {
              status: "error",
              message: error instanceof Error ? error.message : String(error),
            },
          }));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [connectionId, connected, fileStates]);

  if (!connection) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center px-8">
        <Server className="mb-4 h-10 w-10 text-gray-300" />
        <div className="mb-1.5 text-[15px] font-medium text-text-base">
          {t("remote.emptyTitle")}
        </div>
        <div className="text-[13px] text-text-secondary">
          {t("remote.emptyDesc")}
        </div>
      </div>
    );
  }

  if (!connected) {
    const failed = connection.status === "error";
    return (
      <div className="flex h-full w-full flex-col items-center justify-center px-8">
        {failed ? (
          <Server className="mb-4 h-10 w-10 text-gray-300" />
        ) : (
          <Loader2 className="mb-4 h-8 w-8 animate-spin text-gray-400" />
        )}
        <div className="mb-1.5 text-[15px] font-medium text-text-base">
          {failed ? t("remote.connectFailed") : t("remote.connecting")}
        </div>
        <div className="text-[13px] text-text-secondary">
          {connection.username}@{connection.host}:{connection.port}
        </div>
        {failed && (
          <div className="text-[12px] text-gray-400">
            {t("remote.reconnectHint")}
          </div>
        )}
      </div>
    );
  }

  const activeTab = fileTabs.find((tab) => tab.path === activeKey) ?? null;
  const activeState = activeTab ? fileStates[activeTab.path] : null;

  const closeTab = (path: string) => {
    setFileTabs((prev) => prev.filter((tab) => tab.path !== path));
    setFileStates((prev) => {
      const next = { ...prev };
      delete next[path];
      return next;
    });
    setActiveKey((prev) => (prev === path ? TERMINAL_TAB_KEY : prev));
  };

  const tabButtonClassName = (active: boolean) =>
    `flex h-[26px] shrink-0 items-center gap-1.5 rounded-md px-2.5 text-[12px] transition-colors ${
      active
        ? "bg-white text-text-base shadow-sm"
        : "text-text-secondary hover:bg-black/5 hover:text-text-base"
    }`;

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-[34px] shrink-0 items-center gap-1 overflow-x-auto border-b border-border-theme bg-black/[0.03] px-1.5">
        <button
          type="button"
          className={tabButtonClassName(activeKey === TERMINAL_TAB_KEY)}
          onClick={() => setActiveKey(TERMINAL_TAB_KEY)}
        >
          <Terminal className="h-3.5 w-3.5" />
          {t("remote.tabTerminal")}
        </button>
        {fileTabs.map((tab) => (
          <button
            key={tab.path}
            type="button"
            title={tab.path}
            className={tabButtonClassName(activeKey === tab.path)}
            onClick={() => setActiveKey(tab.path)}
          >
            <FileText className="h-3.5 w-3.5 shrink-0" />
            <span className="max-w-[160px] truncate">{tab.name}</span>
            <span
              role="button"
              tabIndex={-1}
              aria-label="close"
              className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full hover:bg-black/10"
              onClick={(event) => {
                event.stopPropagation();
                closeTab(tab.path);
              }}
            >
              <X className="h-3 w-3" />
            </span>
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1">
        <div className={`h-full ${activeTab ? "hidden" : ""}`}>
          <TerminalPlugin mode="remote" connectionId={connection.id} />
        </div>
        {activeTab && (
          <div className="flex h-full flex-col overflow-hidden">
            {activeState?.status === "loading" && (
              <div className="flex flex-1 items-center justify-center text-text-secondary">
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                <span className="text-[13px]">{t("remote.previewLoading")}</span>
              </div>
            )}
            {activeState?.status === "error" && (
              <div className="flex flex-1 items-center justify-center px-8 text-center">
                <div>
                  <div className="mb-1 text-[13px] text-red-500">
                    {t("remote.previewLoadFailed")}
                  </div>
                  <div className="text-[12px] text-gray-400 break-all">
                    {activeState.message}
                  </div>
                </div>
              </div>
            )}
            {activeState?.status === "loaded" && (
              <>
                {activeState.truncated && (
                  <div className="shrink-0 bg-amber-50 px-3 py-1 text-[11px] text-amber-600">
                    {t("remote.previewTruncated")}
                  </div>
                )}
                <div className="min-h-0 flex-1 overflow-auto p-3">
                  <SyntaxHighlightedCode
                    language={languageForFile(activeTab.name)}
                    content={activeState.content}
                    theme="light"
                  />
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
