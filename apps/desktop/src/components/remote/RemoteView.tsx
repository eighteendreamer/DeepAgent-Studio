import { FileWorkspace } from "./FileWorkspace";
import { FileOperationDialog, type FileAction } from "./FileOperationDialog";
import { ExternalEditorPanel } from "./ExternalEditorPanel";
import type { FileEndpoint } from "./filePanelModel";
import { SshForwardPanel } from "./SshForwardPanel";
import { Button } from "../shadcn/button";
import { HoverInfo } from "../ui/HoverInfo";
import { useEffect, useState } from "react";
import { FileText, Loader2, Plus, Server, Terminal, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { TerminalWorkspace, type TerminalOpenIntent } from "../terminal/TerminalWorkspace";
import { PreviewBody } from "../plugins/FilePreviewPlugin";
import { FloatingChat } from "./FloatingChat";
import { RemoteTransfersPanel } from "./RemoteTransfersPanel";
import { SyntaxHighlightedCode } from "../SyntaxHighlightedCode";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "../shadcn/context-menu";
import { sshReadFile, sshReadFileBase64, type SshConnection } from "../../api";

interface RemoteViewProps {
  connection: SshConnection | null;
  openFile: { path: string; name: string; token: number } | null;
}

interface FileTab {
  id: string;
  connectionId: string;
  path: string;
  name: string;
}

const REMOTE_BINARY_PREVIEW_MAX_BYTES = 10 * 1024 * 1024;

type FileTabState =
  | { status: "loading" }
  | { status: "loaded"; kind: "text"; content: string; truncated: boolean }
  | { status: "loaded"; kind: "binary"; blob: Blob }
  | { status: "tooLarge" }
  | { status: "error"; message: string };

const decodeBase64ToBlob = (base64: string): Blob => {
  const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  return new Blob([bytes]);
};

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
  const [fileAction, setFileAction] = useState<FileAction | null>(null);
  const [terminalDirectory, setTerminalDirectory] = useState<{ endpoint: FileEndpoint; path: string; token: number }>();
  const [showForwarding, setShowForwarding] = useState(false);
  const [terminalIntent, setTerminalIntent] = useState<TerminalOpenIntent>();
  const [fileTabs, setFileTabs] = useState<FileTab[]>([]);
  const [activeKey, setActiveKey] = useState<string>("terminal-1");
  const [fileStates, setFileStates] = useState<Record<string, FileTabState>>({});

  const connectionId = connection?.id ?? null;
  const connected = connection?.status === "connected";

  useEffect(() => {
    setTerminalIntent(undefined);
    setShowForwarding(false);
    setFileAction(null); setTerminalDirectory(undefined);
    setFileTabs([]);
    setFileStates({});
    setActiveKey("terminal-1");
  }, [connectionId]);

  const addTerminalTab = (endpoint?: FileEndpoint, cwd?: string) => {
    setTerminalIntent({ token: crypto.randomUUID(), endpoint: endpoint ?? { kind: "ssh", connectionId: connectionId! }, cwd });
    setActiveKey("terminal-1");
  };

  const isTerminalTab = (key: string) => key.startsWith("terminal-");

  const openEndpointFile = (endpointId: string, path: string, name: string) => {
    const id = `file:${endpointId}:${path}`;
    setFileTabs(prev => { if (prev.some(tab => tab.id === id)) return prev; const next = [...prev, { id, connectionId: endpointId, path, name }]; return next.length > MAX_FILE_TABS ? next.slice(-MAX_FILE_TABS) : next; });
    setActiveKey(id);
    setFileStates(prev => {
      const entries = Object.entries(prev); const next = Object.fromEntries(entries.slice(-(MAX_FILE_TABS - 1)));
      return { ...next, [id]: prev[id] ?? { status: "loading" } };
    });
  };
  useEffect(() => { if (connected && openFile && connectionId) openEndpointFile(connectionId, openFile.path, openFile.name); }, [openFile, connected, connectionId]);

  useEffect(() => {
    if (!connectionId || !connected) return;
    const loadingPath = Object.keys(fileStates).find(
      (path) => fileStates[path].status === "loading",
    );
    const loadingTab = fileTabs.find(tab => tab.id === loadingPath);
    if (!loadingPath || !loadingTab) return;
    let cancelled = false;
    const settle = (state: FileTabState) => {
      if (!cancelled) {
        setFileStates((prev) => ({ ...prev, [loadingPath]: state }));
      }
    };
    sshReadFile(loadingTab.connectionId, loadingTab.path)
      .then(async (result) => {
        if (!result.is_binary) {
          settle({
            status: "loaded",
            kind: "text",
            content: result.content,
            truncated: result.truncated,
          });
          return;
        }
        // 二进制（图片/PDF/office 等）：先按远端体积挡掉超限文件，再取全量字节
        if (
          result.size != null &&
          result.size > REMOTE_BINARY_PREVIEW_MAX_BYTES
        ) {
          settle({ status: "tooLarge" });
          return;
        }
        const binary = await sshReadFileBase64(loadingTab.connectionId, loadingTab.path);
        if (binary.truncated || !binary.data_base64) {
          settle({ status: "tooLarge" });
          return;
        }
        settle({
          status: "loaded",
          kind: "binary",
          blob: decodeBase64ToBlob(binary.data_base64),
        });
      })
      .catch((error) => {
        settle({
          status: "error",
          message: error instanceof Error ? error.message : String(error),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [connectionId, connected, fileStates, fileTabs]);

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
      </div>
    );
  }

  const activeTab = fileTabs.find((tab) => tab.id === activeKey) ?? null;
  const activeState = activeTab ? fileStates[activeTab.id] : null;

  const closeTab = (path: string) => {
    setFileTabs((prev) => prev.filter((tab) => tab.id !== path));
    setFileStates((prev) => {
      const next = { ...prev };
      delete next[path];
      return next;
    });
    setActiveKey((prev) => {
      if (prev !== path) return prev;
      return "terminal-1";
    });
  };

  const tabButtonClassName = (active: boolean) =>
    `flex h-[26px] shrink-0 items-center gap-1.5 rounded-md px-2.5 text-[12px] transition-colors ${
      active
        ? "bg-white text-text-base shadow-sm"
        : "text-text-secondary hover:bg-black/5 hover:text-text-base"
    }`;

  return (
    <div className="flex h-full w-full flex-col">
      <div
        className="flex h-[34px] shrink-0 items-center gap-1 overflow-x-auto border-b border-border-theme bg-black/[0.03] px-1.5"
        data-custom-contextmenu="1"
      >
        <Button type="button" variant="ghost" size="sm" aria-pressed={showForwarding} onClick={() => setShowForwarding(value => !value)}>{t("remote.forwarding.title")}</Button>
        <Button variant="ghost" size="sm" onClick={() => setActiveKey("files")}>{t("remote.files.title")}</Button>
        <Button size="sm" variant={isTerminalTab(activeKey) ? "secondary" : "ghost"} onClick={() => setActiveKey("terminal-1")}><Terminal className="h-3.5 w-3.5" />{t("remote.tabTerminal")}</Button>
        <HoverInfo content={t("remote.tabNewTerminal")}><button
          type="button"
          className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-black/5 hover:text-text-base"
          onClick={() => addTerminalTab()}
          aria-label={t("remote.tabNewTerminal")}
        >
          <Plus className="h-3.5 w-3.5" />
        </button></HoverInfo>
        {fileTabs.map((tab) => (
          <ContextMenu key={tab.id}>
            <ContextMenuTrigger asChild>
              <HoverInfo content={tab.path}><button
                type="button"

                className={tabButtonClassName(activeKey === tab.id)}
                onClick={() => setActiveKey(tab.id)}
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
                    closeTab(tab.id);
                  }}
                >
                  <X className="h-3 w-3" />
                </span>
              </button></HoverInfo>
            </ContextMenuTrigger>
            <ContextMenuContent>
              <ContextMenuItem onSelect={() => closeTab(tab.id)}>
                {t("remote.tabClose")}
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        ))}
      </div>
      {showForwarding && <SshForwardPanel key={connection.id} connectionId={connection.id} />}
      <div className="min-h-0 flex-1">
        <div className="h-full" style={{ display: activeKey === "files" ? undefined : "none" }}><FileWorkspace key={connection.id} connection={connection} terminalDirectory={terminalDirectory} onOpenTerminal={(endpoint, cwd) => addTerminalTab(endpoint, cwd)} onOpenFile={openEndpointFile} onCopy={(source, destination) => setFileAction({ kind: "copy", source, destination })} onExternalEdit={(connectionId, path) => setFileAction({ kind: "editor", connectionId, path })} onExtract={(connectionId, path, directory) => setFileAction({ kind: "extract", connectionId, path, directory })} /></div>
        <div className="h-full" style={{ display: isTerminalTab(activeKey) ? undefined : "none" }}>
          <TerminalWorkspace key={connection.id} mode="remote" connectionId={connection.id} active={isTerminalTab(activeKey)} openIntent={terminalIntent} onDirectoryChange={(endpoint, path) => setTerminalDirectory({ endpoint, path, token: Date.now() })} />
        </div>
        {activeTab && !isTerminalTab(activeKey) && (
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
            {activeState?.status === "tooLarge" && (
              <div className="flex flex-1 items-center justify-center px-8 text-center">
                <div className="text-[13px] text-text-secondary">
                  {t("remote.previewTooLarge")}
                </div>
              </div>
            )}
            {activeState?.status === "loaded" && activeState.kind === "text" && (
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
            {activeState?.status === "loaded" && activeState.kind === "binary" && (
              <div className="min-h-0 flex-1 overflow-hidden bg-white">
                <PreviewBody fileBlob={activeState.blob} fileName={activeTab.name} />
              </div>
            )}
          </div>
        )}
      </div>
      <FloatingChat connectionId={connection.id} connected={connected} />
      <ExternalEditorPanel />
      <RemoteTransfersPanel connectionId={connection.id}/>
      {fileAction && <FileOperationDialog key={JSON.stringify(fileAction)} action={fileAction} onClose={() => setFileAction(null)} />}
    </div>
  );
}
