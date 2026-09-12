import {
  ArrowLeft,
  Check,
  ChevronDown,
  ChevronRight,
  File as FileIcon,
  FilePlus,
  Folder,
  FolderOpen,
  FolderPlus,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Server,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { message } from "../message";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "../shadcn/context-menu";
import { normalizeSshError } from "../settings/ConnectionsSettings";
import {
  pickSshIdentityFile,
  sshConnect,
  sshCreateConnection,
  sshCreateDir,
  sshCreateFile,
  sshListConnections,
  sshListDir,
  sshPushFile,
  sshRemoveConnection,
  sshRemovePath,
  sshRenamePath,
  sshUpdateConnection,
  type SshConnection,
  type SshDirEntry,
} from "../../api";

type AuthType = "password" | "file";

type ManagerMode = "list" | "create" | "edit";

interface RemoteSidebarProps {
  selected: SshConnection | null;
  onSelect: (conn: SshConnection) => void;
  onOpenFile: (path: string, name: string) => void;
  onBack: () => void;
}

interface ConnectionForm {
  name: string;
  host: string;
  port: string;
  username: string;
  authType: AuthType;
  keyPath: string;
  password: string;
}

const emptyForm: ConnectionForm = {
  name: "",
  host: "",
  port: "22",
  username: "",
  authType: "password",
  keyPath: "",
  password: "",
};

const statusDotClassName = (status: SshConnection["status"]) => {
  switch (status) {
    case "connected":
      return "bg-green-500";
    case "connecting":
      return "bg-yellow-400";
    case "error":
      return "bg-red-400";
    default:
      return "bg-gray-300";
  }
};

type InlineEdit =
  | { kind: "create"; parentDir: string; entryType: "dir" | "file" }
  | { kind: "rename"; entry: SshDirEntry };

const parentDirOf = (path: string, name: string) => {
  const idx = path.length - name.length - 1;
  return idx > 0 ? path.slice(0, idx) : "/";
};

const joinRemotePath = (dir: string, name: string) =>
  dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`;

const validateTreeName = (value: string) => {
  const name = value.trim();
  return name && !name.includes("/") ? name : null;
};

// 新建/重命名的行内输入框：Enter 提交，Escape 取消，失焦按提交处理；
// once-guard 防止 Enter 后紧跟的 blur 造成重复提交。
function InlineNameInput({
  defaultValue,
  placeholder,
  depth,
  onCommit,
  onCancel,
}: {
  defaultValue: string;
  placeholder: string;
  depth: number;
  onCommit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(defaultValue);
  const doneRef = useRef(false);
  const commit = () => {
    if (doneRef.current) return;
    doneRef.current = true;
    onCommit(value);
  };
  const cancel = () => {
    if (doneRef.current) return;
    doneRef.current = true;
    onCancel();
  };
  return (
    <div className="py-0.5 pr-1.5" style={{ paddingLeft: 4 + depth * 12 }}>
      <input
        autoFocus
        type="text"
        value={value}
        onChange={(event) => setValue(event.target.value)}
        onFocus={(event) => event.target.select()}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          } else if (event.key === "Escape") {
            event.preventDefault();
            cancel();
          }
        }}
        onBlur={commit}
        placeholder={placeholder}
        className="h-[22px] w-full rounded-md border border-blue-500 bg-white px-1.5 text-[12px] text-text-base outline-none"
      />
    </div>
  );
}

export function RemoteSidebar({
  selected,
  onSelect,
  onOpenFile,
  onBack,
}: RemoteSidebarProps) {
  const { t } = useTranslation();
  const [connections, setConnections] = useState<SshConnection[]>([]);
  const [connectingId, setConnectingId] = useState<string | null>(null);
  const [managerOpen, setManagerOpen] = useState(false);
  const [managerMode, setManagerMode] = useState<ManagerMode>("list");
  const [form, setForm] = useState<ConnectionForm>(emptyForm);
  const [filter, setFilter] = useState("");
  const [tree, setTree] = useState<Record<string, SshDirEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loadingDir, setLoadingDir] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);
  const [uploads, setUploads] = useState<
    Array<{
      id: string;
      name: string;
      status: "uploading" | "success" | "error";
      error?: string;
    }>
  >([]);
  const [inlineEdit, setInlineEdit] = useState<InlineEdit | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState<string | null>(null);
  const [editingConn, setEditingConn] = useState<SshConnection | null>(null);

  const selectedRef = useRef(selected);
  selectedRef.current = selected;

  const loadConnections = () => {
    sshListConnections()
      .then(setConnections)
      .catch(() => setConnections([]));
  };

  useEffect(() => {
    loadConnections();
  }, []);

  // Tauri native drag-drop event handler for file uploads.
  // Registered once; latest connection is read via selectedRef to avoid
  // re-registering (which races async setup and leaks duplicate listeners).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const resolveDropTarget = (position: { x: number; y: number }): string => {
      const el = document.elementFromPoint(
        position.x / window.devicePixelRatio,
        position.y / window.devicePixelRatio,
      );
      const target = el?.closest<HTMLElement>("[data-remote-drop-dir]");
      return target?.dataset.remoteDropDir || "/";
    };

    const setupDragDrop = async () => {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      if (disposed) return;
      const unlistenFn = await getCurrentWebview().onDragDropEvent(async (event) => {
        if (event.payload.type === "drop") {
          const paths = event.payload.paths;
          if (paths.length === 0) return;

          const conn = selectedRef.current;
          if (!conn) {
            message.error(t("remote.notConnected"));
            return;
          }

          const targetDir = resolveDropTarget(event.payload.position);

          const newUploads = paths.map((filePath) => {
            const fileName = filePath.split(/[\\/]/).pop() || filePath;
            return {
              id: `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`,
              name: fileName,
              status: "uploading" as const,
            };
          });

          setUploads((prev) => [...prev, ...newUploads]);

          let successCount = 0;
          let errorCount = 0;

          for (const upload of newUploads) {
            const localPath = paths.find((p) =>
              p.split(/[\\/]/).pop() === upload.name,
            );
            if (!localPath) {
              setUploads((prev) =>
                prev.map((u) =>
                  u.id === upload.id
                    ? { ...u, status: "error" as const, error: "无法获取文件路径" }
                    : u,
                ),
              );
              errorCount++;
              continue;
            }

            const remotePath = targetDir.endsWith("/")
              ? `${targetDir}${upload.name}`
              : `${targetDir}/${upload.name}`;

            try {
              await sshPushFile(conn.id, {
                local_path: localPath,
                remote_path: remotePath,
                create_parent: true,
                overwrite: true,
                verify_mode: "size",
              });
              setUploads((prev) =>
                prev.map((u) =>
                  u.id === upload.id ? { ...u, status: "success" as const } : u,
                ),
              );
              successCount++;
            } catch (error) {
              const errorMsg =
                error instanceof Error ? error.message : String(error);
              setUploads((prev) =>
                prev.map((u) =>
                  u.id === upload.id
                    ? { ...u, status: "error" as const, error: errorMsg }
                    : u,
                ),
              );
              errorCount++;
            }
          }

          await loadDir(conn.id, targetDir);

          if (successCount > 0) {
            message.success(`已上传 ${successCount} 个文件到 ${targetDir}`);
          }
          if (errorCount > 0) {
            message.error(`${errorCount} 个文件上传失败`);
          }

          setTimeout(() => {
            setUploads((prev) => prev.filter((u) => u.status === "uploading"));
          }, 10000);
        }
      });
      if (disposed) {
        unlistenFn();
        return;
      }
      unlisten = unlistenFn;
    };

    void setupDragDrop();

    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [t]);

  const loadDir = async (connectionId: string, path: string) => {
    setLoadingDir(path);
    try {
      const listing = await sshListDir(connectionId, path);
      setTree((prev) => ({ ...prev, [path]: listing.entries }));
      setDirError(null);
    } catch (error) {
      setDirError(
        normalizeSshError(
          error instanceof Error ? error.message : String(error),
          t,
        ),
      );
    } finally {
      setLoadingDir(null);
    }
  };

  const handleSelect = async (conn: SshConnection) => {
    setManagerOpen(false);
    onSelect(conn);
    setTree({});
    setExpanded(new Set());
    setDirError(null);
    setFilter("");
    let current = conn;
    if (conn.status !== "connected") {
      setConnectingId(conn.id);
      try {
        await sshConnect(conn.id);
      } catch (error) {
        const displayError = normalizeSshError(
          error instanceof Error ? error.message : String(error),
          t,
        );
        message.error(`${t("remote.connectFailed")}: ${displayError}`);
      } finally {
        setConnectingId(null);
      }
      const list = await sshListConnections().catch(() => []);
      setConnections(list);
      const refreshed = list.find((item) => item.id === conn.id);
      if (refreshed) {
        current = refreshed;
        onSelect(refreshed);
      }
      if (current.status !== "connected") return;
    }
    await loadDir(current.id, "/");
  };

  // 重进远程视图时本组件重挂载、目录树状态已丢失，而 selected 是 App 级
  // 持久状态；挂载时复用 handleSelect 重载根目录（含断线重连路径）。
  useEffect(() => {
    const conn = selectedRef.current;
    if (conn) void handleSelect(conn);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggleDir = (entry: SshDirEntry) => {
    const next = new Set(expanded);
    if (next.has(entry.path)) {
      next.delete(entry.path);
      setExpanded(next);
      return;
    }
    next.add(entry.path);
    setExpanded(next);
    if (selected && !tree[entry.path]) void loadDir(selected.id, entry.path);
  };

  const reportTreeError = (error: unknown) => {
    message.error(
      `${t("remote.treeOpFailed")}: ${normalizeSshError(
        error instanceof Error ? error.message : String(error),
        t,
      )}`,
    );
  };

  const startCreate = (parentDir: string, entryType: "dir" | "file") => {
    setConfirmingDelete(null);
    setInlineEdit({ kind: "create", parentDir, entryType });
  };

  const startRename = (entry: SshDirEntry) => {
    setConfirmingDelete(null);
    setInlineEdit({ kind: "rename", entry });
  };

  const handleCreateConfirm = async (value: string) => {
    const edit = inlineEdit;
    const conn = selected;
    const name = validateTreeName(value);
    setInlineEdit(null);
    if (!edit || edit.kind !== "create" || !conn || !name) return;
    const targetPath = joinRemotePath(edit.parentDir, name);
    try {
      if (edit.entryType === "dir") {
        await sshCreateDir(conn.id, targetPath);
      } else {
        await sshCreateFile(conn.id, targetPath);
      }
      await loadDir(conn.id, edit.parentDir);
    } catch (error) {
      reportTreeError(error);
    }
  };

  const handleRenameConfirm = async (value: string) => {
    const edit = inlineEdit;
    const conn = selected;
    const name = validateTreeName(value);
    setInlineEdit(null);
    if (
      !edit ||
      edit.kind !== "rename" ||
      !conn ||
      !name ||
      name === edit.entry.name
    )
      return;
    const parentDir = parentDirOf(edit.entry.path, edit.entry.name);
    try {
      await sshRenamePath(
        conn.id,
        edit.entry.path,
        joinRemotePath(parentDir, name),
      );
      setExpanded((prev) => {
        if (!prev.has(edit.entry.path)) return prev;
        const next = new Set(prev);
        next.delete(edit.entry.path);
        return next;
      });
      setTree((prev) => {
        if (!prev[edit.entry.path]) return prev;
        const next = { ...prev };
        delete next[edit.entry.path];
        return next;
      });
      await loadDir(conn.id, parentDir);
    } catch (error) {
      reportTreeError(error);
    }
  };

  const handleDelete = async (entry: SshDirEntry) => {
    const conn = selected;
    if (!conn) return;
    const parentDir = parentDirOf(entry.path, entry.name);
    try {
      await sshRemovePath(conn.id, entry.path);
      setExpanded((prev) => {
        if (!prev.has(entry.path)) return prev;
        const next = new Set(prev);
        next.delete(entry.path);
        return next;
      });
      setTree((prev) => {
        if (!prev[entry.path]) return prev;
        const next = { ...prev };
        delete next[entry.path];
        return next;
      });
      await loadDir(conn.id, parentDir);
    } catch (error) {
      reportTreeError(error);
    }
  };

  const handlePickKeyFile = async () => {
    const selectedPath = await pickSshIdentityFile();
    if (!selectedPath) return;
    setForm((prev) => ({ ...prev, keyPath: selectedPath }));
  };

  const handleSave = async () => {
    if (!form.name || !form.host || !form.username) return;
    const isEditing = managerMode === "edit";
    const authType = form.authType === "file" ? "key_file" : "password";
    const keyPath = form.authType === "file" ? form.keyPath : undefined;
    const password = form.authType === "password" ? form.password : undefined;
    try {
      if (isEditing && editingConn) {
        await sshUpdateConnection(
          editingConn.id,
          form.name,
          form.host,
          parseInt(form.port, 10) || 22,
          form.username,
          authType,
          keyPath,
          password,
        );
      } else {
        const created = await sshCreateConnection(
          form.name,
          form.host,
          parseInt(form.port, 10) || 22,
          form.username,
          authType,
          keyPath,
          password,
        );
        void handleSelect(created);
      }
      setForm(emptyForm);
      setEditingConn(null);
      setManagerMode("list");
      loadConnections();
    } catch (error) {
      const displayError = normalizeSshError(
        error instanceof Error ? error.message : String(error),
        t,
      );
      message.error(
        `${t(isEditing ? "remote.updateFailed" : "remote.createFailed")}: ${displayError}`,
      );
    }
  };

  // 与设置页 ConnectionsSettings.openEdit 同一预填规则：密码不回显，
  // 密码型连接保存时必须重填（validate_config 拒绝空密码）。
  const startEdit = (conn: SshConnection) => {
    setForm({
      name: conn.name,
      host: conn.host,
      port: String(conn.port),
      username: conn.username,
      authType: conn.key_path ? "file" : "password",
      keyPath: conn.key_path || "",
      password: "",
    });
    setEditingConn(conn);
    setManagerMode("edit");
  };

  const handleDeleteConnection = async (conn: SshConnection) => {
    try {
      await sshRemoveConnection(conn.id);
      loadConnections();
    } catch (error) {
      const displayError = normalizeSshError(
        error instanceof Error ? error.message : String(error),
        t,
      );
      message.error(`${t("remote.deleteFailed")}: ${displayError}`);
    }
  };

  const statusLabel = (status: SshConnection["status"]) => {
    switch (status) {
      case "connected":
        return t("settings.connections.online");
      case "connecting":
        return t("settings.connections.checking");
      default:
        return t("settings.connections.offline");
    }
  };

  // 两步确认删除：第一次点击 preventDefault 保持菜单打开并切换为确认文案。
  const deleteMenuItem = (entry: SshDirEntry, isDir: boolean) => (
    <ContextMenuItem
      className="text-red-500"
      onSelect={(event) => {
        if (confirmingDelete !== entry.path) {
          event.preventDefault();
          setConfirmingDelete(entry.path);
          return;
        }
        setConfirmingDelete(null);
        void handleDelete(entry);
      }}
    >
      <Trash2 className="mr-2 h-3.5 w-3.5" />
      {confirmingDelete === entry.path
        ? t(isDir ? "remote.treeConfirmDeleteDir" : "remote.treeConfirmDelete")
        : t("remote.treeDelete")}
    </ContextMenuItem>
  );

  const dirMenuItems = (entry: SshDirEntry) => (
    <>
      <ContextMenuItem onSelect={() => startCreate(entry.path, "dir")}>
        <FolderPlus className="mr-2 h-3.5 w-3.5 text-text-secondary" />
        {t("remote.treeNewFolder")}
      </ContextMenuItem>
      <ContextMenuItem onSelect={() => startCreate(entry.path, "file")}>
        <FilePlus className="mr-2 h-3.5 w-3.5 text-text-secondary" />
        {t("remote.treeNewFile")}
      </ContextMenuItem>
      <ContextMenuItem
        onSelect={() => {
          if (selected) void loadDir(selected.id, entry.path);
        }}
      >
        <RefreshCw className="mr-2 h-3.5 w-3.5 text-text-secondary" />
        {t("remote.treeRefresh")}
      </ContextMenuItem>
      <ContextMenuItem onSelect={() => startRename(entry)}>
        <Pencil className="mr-2 h-3.5 w-3.5 text-text-secondary" />
        {t("remote.treeRename")}
      </ContextMenuItem>
      <ContextMenuSeparator />
      {deleteMenuItem(entry, true)}
    </>
  );

  const fileMenuItems = (entry: SshDirEntry) => (
    <>
      <ContextMenuItem onSelect={() => startRename(entry)}>
        <Pencil className="mr-2 h-3.5 w-3.5 text-text-secondary" />
        {t("remote.treeRename")}
      </ContextMenuItem>
      <ContextMenuSeparator />
      {deleteMenuItem(entry, false)}
    </>
  );

  const renderRows = (entries: SshDirEntry[], depth: number) => (
    <>
      {entries.map((entry) => {
        const rowClassName =
          "flex w-full items-center gap-1 rounded-md py-1 pr-1.5 text-left text-[12px] text-text-base hover:bg-black/5";
        if (entry.is_dir) {
          const isOpen = expanded.has(entry.path);
          const children = tree[entry.path];
          const isRenaming =
            inlineEdit?.kind === "rename" && inlineEdit.entry.path === entry.path;
          const isCreating =
            inlineEdit?.kind === "create" && inlineEdit.parentDir === entry.path;
          return (
            <div key={entry.path} data-custom-contextmenu="1">
              {isRenaming ? (
                <InlineNameInput
                  defaultValue={entry.name}
                  placeholder={t("remote.treeNamePlaceholder")}
                  depth={depth}
                  onCommit={(value) => void handleRenameConfirm(value)}
                  onCancel={() => setInlineEdit(null)}
                />
              ) : (
                <ContextMenu
                  onOpenChange={(open) => {
                    if (!open) setConfirmingDelete(null);
                  }}
                >
                  <ContextMenuTrigger asChild>
                    <button
                      type="button"
                      className={rowClassName}
                      style={{ paddingLeft: 4 + depth * 12 }}
                      title={entry.path}
                      onClick={() => toggleDir(entry)}
                      data-remote-drop-dir={entry.path}
                    >
                      {isOpen ? (
                        <ChevronDown className="h-3 w-3 shrink-0 text-text-secondary" />
                      ) : (
                        <ChevronRight className="h-3 w-3 shrink-0 text-text-secondary" />
                      )}
                      {isOpen ? (
                        <FolderOpen className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                      ) : (
                        <Folder className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                      )}
                      <span className="truncate">{entry.name}</span>
                      {loadingDir === entry.path && (
                        <Loader2 className="h-3 w-3 shrink-0 animate-spin text-text-secondary" />
                      )}
                    </button>
                  </ContextMenuTrigger>
                  <ContextMenuContent
                    onCloseAutoFocus={(event) => event.preventDefault()}
                  >
                    {dirMenuItems(entry)}
                  </ContextMenuContent>
                </ContextMenu>
              )}
              {isCreating && (
                <InlineNameInput
                  defaultValue=""
                  placeholder={t("remote.treeNamePlaceholder")}
                  depth={depth + 1}
                  onCommit={(value) => void handleCreateConfirm(value)}
                  onCancel={() => setInlineEdit(null)}
                />
              )}
              {isOpen && children && renderRows(children, depth + 1)}
            </div>
          );
        }
        const isRenaming =
          inlineEdit?.kind === "rename" && inlineEdit.entry.path === entry.path;
        return (
          <div key={entry.path} data-custom-contextmenu="1">
            {isRenaming ? (
              <InlineNameInput
                defaultValue={entry.name}
                placeholder={t("remote.treeNamePlaceholder")}
                depth={depth}
                onCommit={(value) => void handleRenameConfirm(value)}
                onCancel={() => setInlineEdit(null)}
              />
            ) : (
              <ContextMenu
                onOpenChange={(open) => {
                  if (!open) setConfirmingDelete(null);
                }}
              >
                <ContextMenuTrigger asChild>
                  <button
                    type="button"
                    className="flex w-full items-center gap-1 rounded-md py-1 pr-1.5 pl-0 text-left text-[12px] text-text-base hover:bg-black/5"
                    style={{ paddingLeft: 4 + depth * 12 }}
                    title={entry.path}
                    onClick={() => onOpenFile(entry.path, entry.name)}
                  >
                    <span className="w-3 shrink-0" />
                    <FileIcon className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                    <span className="truncate">{entry.name}</span>
                  </button>
                </ContextMenuTrigger>
                <ContextMenuContent
                  onCloseAutoFocus={(event) => event.preventDefault()}
                >
                  {fileMenuItems(entry)}
                </ContextMenuContent>
              </ContextMenu>
            )}
          </div>
        );
      })}
    </>
  );

  const rootEntries = tree["/"];
  const query = filter.trim().toLowerCase();
  const searchMatches = query
    ? Object.entries(tree).flatMap(([dir, entries]) =>
        entries
          .filter((entry) => entry.name.toLowerCase().includes(query))
          .map((entry) => ({ dir, entry })),
      )
    : [];

  // 连接行删除：与目录树删除同一两步确认交互；键用 conn.id（树键以 / 开头，不冲突）。
  const deleteConnectionMenuItem = (conn: SshConnection) => (
    <ContextMenuItem
      className="text-red-500"
      onSelect={(event) => {
        if (confirmingDelete !== conn.id) {
          event.preventDefault();
          setConfirmingDelete(conn.id);
          return;
        }
        setConfirmingDelete(null);
        void handleDeleteConnection(conn);
      }}
    >
      <Trash2 className="mr-2 h-3.5 w-3.5" />
      {confirmingDelete === conn.id
        ? t("remote.connConfirmDelete")
        : t("remote.treeDelete")}
    </ContextMenuItem>
  );

  const connectionRows = (
    <div className="space-y-0.5">
      {connections.length === 0 ? (
        <div className="px-2.5 py-1 text-[12px] text-text-secondary">
          {t("remote.noConnections")}
        </div>
      ) : (
        connections.map((conn) => (
          <ContextMenu
            key={conn.id}
            onOpenChange={(open) => {
              if (!open) setConfirmingDelete(null);
            }}
          >
            <ContextMenuTrigger asChild>
              <button
                type="button"
                disabled={connectingId === conn.id}
                onClick={() => void handleSelect(conn)}
                data-custom-contextmenu="1"
                className={`flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left transition-colors hover:bg-black/5 disabled:opacity-60 ${
                  selected?.id === conn.id ? "bg-sidebar-highlight" : ""
                }`}
              >
                <Server className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[12px] text-text-base">
                    {conn.name}
                  </span>
                  <span className="block truncate text-[10px] text-text-secondary">
                    {conn.username}@{conn.host}:{conn.port}
                  </span>
                </span>
                {connectingId === conn.id ? (
                  <Loader2 className="h-3 w-3 shrink-0 animate-spin text-text-secondary" />
                ) : (
                  <span
                    className={`h-1.5 w-1.5 shrink-0 rounded-full ${statusDotClassName(conn.status)}`}
                    title={statusLabel(conn.status)}
                  />
                )}
              </button>
            </ContextMenuTrigger>
            <ContextMenuContent
              onCloseAutoFocus={(event) => event.preventDefault()}
            >
              <ContextMenuItem onSelect={() => startEdit(conn)}>
                <Pencil className="mr-2 h-3.5 w-3.5 text-text-secondary" />
                {t("remote.connEdit")}
              </ContextMenuItem>
              <ContextMenuSeparator />
              {deleteConnectionMenuItem(conn)}
            </ContextMenuContent>
          </ContextMenu>
        ))
      )}
    </div>
  );

  return (
    <aside className="flex h-full w-[240px] flex-shrink-0 flex-col bg-sidebar-bg no-select pb-2">
      <div className="px-3 pt-4 pb-3">
        <button
          type="button"
          onClick={onBack}
          className="flex items-center px-2 text-[13px] font-medium text-text-secondary transition-colors hover:text-text-base"
        >
          <ArrowLeft className="mr-2 h-4 w-4" />
          {t("remote.back")}
        </button>
      </div>

      <div className="px-3 pb-2">
        <button
          type="button"
          onClick={() => {
            setManagerMode("list");
            setManagerOpen(true);
          }}
          className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-[13px] font-medium text-text-base transition-colors hover:bg-black/5"
        >
          <Server className="h-4 w-4 shrink-0 text-text-secondary" />
          <span className="min-w-0 flex-1 truncate text-left">
            {selected ? selected.name : t("remote.selectConnection")}
          </span>
          {connectingId === selected?.id ? (
            <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-text-secondary" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
          )}
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-3">
        <div className="mb-1 flex items-center gap-1.5 px-2.5 text-[11px] font-medium text-text-secondary">
          <Search className="h-3 w-3" />
          {t("remote.searchFiles")}
        </div>
        <input
          type="text"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder={t("remote.searchPlaceholder")}
          className="mb-2 h-[28px] w-full rounded-[10px] border border-border-theme bg-white px-2.5 text-[12px] text-text-base outline-none transition-colors focus:border-blue-500"
        />

        {dirError && (
          <div className="mb-2 rounded-md bg-red-50 px-2.5 py-1.5 text-[11px] text-red-500">
            {t("remote.loadFailed")}: {dirError}
          </div>
        )}

        <div className="text-[12px]">
          {!selected ? (
            <div className="px-2.5 py-1 text-text-secondary">
              {t("remote.emptyTree")}
            </div>
          ) : query ? (
            searchMatches.length === 0 ? (
              <div className="px-2.5 py-1 text-text-secondary">
                {t("remote.noMatch")}
              </div>
            ) : (
              searchMatches.map(({ dir, entry }) =>
                entry.is_dir ? (
                  <button
                    key={`${dir}/${entry.name}`}
                    type="button"
                    className="flex w-full items-center gap-1.5 rounded-md px-2.5 py-1 text-left hover:bg-black/5"
                    title={`${dir} · ${entry.path}`}
                    onClick={() => toggleDir(entry)}
                  >
                    <Folder className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                    <span className="min-w-0">
                      <span className="block truncate text-[12px] text-text-base">
                        {entry.name}
                      </span>
                      <span className="block truncate text-[10px] text-text-secondary">
                        {dir}
                      </span>
                    </span>
                  </button>
                ) : (
                  <button
                    key={`${dir}/${entry.name}`}
                    type="button"
                    className="flex w-full items-center gap-1.5 rounded-md px-2.5 py-1 text-left hover:bg-black/5"
                    title={`${dir} · ${entry.path}`}
                    onClick={() => onOpenFile(entry.path, entry.name)}
                  >
                    <FileIcon className="h-3.5 w-3.5 shrink-0 text-text-secondary" />
                    <span className="min-w-0">
                      <span className="block truncate text-[12px] text-text-base">
                        {entry.name}
                      </span>
                      <span className="block truncate text-[10px] text-text-secondary">
                        {dir}
                      </span>
                    </span>
                  </button>
                ),
              )
            )
          ) : rootEntries === undefined ? (
            <div className="flex items-center gap-1.5 px-2.5 py-1 text-text-secondary">
              <Loader2 className="h-3 w-3 animate-spin" />
              {t("settings.connections.checking")}
            </div>
          ) : (
            renderRows(rootEntries, 0)
          )}
          {selected && !query && (
            <ContextMenu
              onOpenChange={(open) => {
                if (!open) setConfirmingDelete(null);
              }}
            >
              <ContextMenuTrigger asChild>
                <div className="min-h-[96px]" data-custom-contextmenu="1" />
              </ContextMenuTrigger>
              <ContextMenuContent
                onCloseAutoFocus={(event) => event.preventDefault()}
              >
                <ContextMenuItem onSelect={() => startCreate("/", "dir")}>
                  <FolderPlus className="mr-2 h-3.5 w-3.5 text-text-secondary" />
                  {t("remote.treeNewFolder")}
                </ContextMenuItem>
                <ContextMenuItem onSelect={() => startCreate("/", "file")}>
                  <FilePlus className="mr-2 h-3.5 w-3.5 text-text-secondary" />
                  {t("remote.treeNewFile")}
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() => {
                    if (selected) void loadDir(selected.id, "/");
                  }}
                >
                  <RefreshCw className="mr-2 h-3.5 w-3.5 text-text-secondary" />
                  {t("remote.treeRefresh")}
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          )}
        </div>
      </div>

      {uploads.length > 0 && (
        <div className="mx-3 mt-2 border-t border-border-theme pt-2">
          <div className="mb-1.5 flex items-center gap-1.5 px-2.5 text-[11px] font-medium text-text-secondary">
            <Upload className="h-3 w-3" />
            {t("remote.uploads")} ({uploads.length})
          </div>
          <div className="max-h-[120px] space-y-1 overflow-y-auto">
            {uploads.map((upload) => (
              <div
                key={upload.id}
                className="flex items-center gap-2 rounded-md px-2.5 py-1 text-[11px]"
              >
                {upload.status === "uploading" && (
                  <Loader2 className="h-3 w-3 shrink-0 animate-spin text-blue-500" />
                )}
                {upload.status === "success" && (
                  <Check className="h-3 w-3 shrink-0 text-green-500" />
                )}
                {upload.status === "error" && (
                  <X className="h-3 w-3 shrink-0 text-red-500" />
                )}
                <span className="min-w-0 flex-1 truncate text-text-base">
                  {upload.name}
                </span>
                {upload.status === "error" && upload.error && (
                  <span className="shrink-0 text-[10px] text-red-400" title={upload.error}>
                    失败
                  </span>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {managerOpen &&
        createPortal(
          <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-[rgba(15,23,42,0.16)] px-6 py-8"
            onMouseDown={() => setManagerOpen(false)}
          >
            <div
              className="flex max-h-[calc(100vh-64px)] w-full max-w-[520px] flex-col overflow-hidden rounded-[20px] bg-elevated-bg shadow-[0_6px_24px_rgba(0,0,0,0.10)]"
              onMouseDown={(event) => event.stopPropagation()}
            >
              {managerMode === "list" ? (
                <>
                  <div className="flex items-start justify-between gap-3 border-b border-border-theme px-5 py-4">
                    <h3 className="text-[18px] font-semibold tracking-tight text-text-base">
                      {t("remote.selectConnection")}
                    </h3>
                    <button
                      type="button"
                      className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-gray-400 transition-colors hover:bg-black/5 hover:text-text-base"
                      onClick={() => setManagerOpen(false)}
                    >
                      ×
                    </button>
                  </div>
                  <div className="min-h-0 flex-1 overflow-y-auto px-5 py-3">
                    {connectionRows}
                  </div>
                  <div className="flex justify-start border-t border-border-theme px-5 py-3">
                    <button
                      type="button"
                      onClick={() => {
                        setForm(emptyForm);
                        setManagerMode("create");
                      }}
                      className="flex items-center gap-1.5 rounded-full bg-black/5 px-4 py-1.5 text-[13px] text-text-base transition-colors hover:bg-black/10"
                    >
                      <Plus className="h-3.5 w-3.5" />
                      {t("remote.newConnection")}
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <div className="flex items-start justify-between gap-3 border-b border-border-theme px-5 py-4">
                    <h3 className="text-[18px] font-semibold tracking-tight text-text-base">
                      {t(
                        managerMode === "edit"
                          ? "remote.editConnection"
                          : "remote.newConnection",
                      )}
                    </h3>
                    <button
                      type="button"
                      className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-gray-400 transition-colors hover:bg-black/5 hover:text-text-base"
                      onClick={() => setManagerOpen(false)}
                    >
                      ×
                    </button>
                  </div>
                  <form
                    className="min-h-0 flex-1 overflow-y-auto"
                    onSubmit={(event) => {
                      event.preventDefault();
                      void handleSave();
                    }}
                  >
                    <div className="space-y-3 px-5 py-4">
                      <div>
                        <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                          {t("settings.connections.displayName")}
                        </label>
                        <input
                          type="text"
                          value={form.name}
                          onChange={(event) =>
                            setForm({ ...form, name: event.target.value })
                          }
                          className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                        />
                      </div>
                      <div>
                        <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                          {t("settings.connections.hostname")}
                        </label>
                        <input
                          type="text"
                          value={form.host}
                          onChange={(event) =>
                            setForm({ ...form, host: event.target.value })
                          }
                          placeholder={t("settings.connections.hostPlaceholder")}
                          className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                        />
                      </div>
                      <div className="grid gap-3 sm:grid-cols-[110px_minmax(0,1fr)]">
                        <div>
                          <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                            {t("settings.connections.sshPort")}{" "}
                            <span className="font-normal text-gray-400">
                              {t("settings.connections.optional")}
                            </span>
                          </label>
                          <input
                            type="text"
                            value={form.port}
                            onChange={(event) =>
                              setForm({ ...form, port: event.target.value })
                            }
                            className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                          />
                        </div>
                        <div>
                          <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                            {t("settings.connections.username")}
                          </label>
                          <input
                            type="text"
                            value={form.username}
                            onChange={(event) =>
                              setForm({ ...form, username: event.target.value })
                            }
                            className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                          />
                        </div>
                      </div>

                      <section className="space-y-2.5 border-t border-border-theme pt-3">
                        <div className="flex items-center gap-4 text-[12px]">
                          <label className="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name="remote-auth"
                              checked={form.authType === "password"}
                              onChange={() =>
                                setForm({ ...form, authType: "password" })
                              }
                            />
                            {t("settings.connections.password")}
                          </label>
                          <label className="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name="remote-auth"
                              checked={form.authType === "file"}
                              onChange={() =>
                                setForm({ ...form, authType: "file" })
                              }
                            />
                            {t("settings.connections.identityFile")}
                          </label>
                        </div>

                        {form.authType === "password" ? (
                          <div>
                            <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                              {t("settings.connections.password")}
                            </label>
                            <input
                              type="password"
                              value={form.password}
                              onChange={(event) =>
                                setForm({ ...form, password: event.target.value })
                              }
                              placeholder={
                                managerMode === "edit"
                                  ? t("remote.editPasswordHint")
                                  : undefined
                              }
                              className="h-[34px] w-full rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                            />
                          </div>
                        ) : (
                          <div>
                            <label className="mb-1.5 block text-[11px] font-medium text-text-base">
                              {t("settings.connections.identityFilePath")}
                            </label>
                            <div className="flex gap-2">
                              <input
                                type="text"
                                value={form.keyPath}
                                onChange={(event) =>
                                  setForm({ ...form, keyPath: event.target.value })
                                }
                                className="h-[34px] min-w-0 flex-1 rounded-[14px] border border-border-theme bg-white px-3 text-[13px] text-text-base outline-none transition-colors focus:border-blue-500"
                              />
                              <button
                                type="button"
                                onClick={() => void handlePickKeyFile()}
                                className="h-[34px] shrink-0 rounded-[14px] bg-black/5 px-3 text-[12px] text-text-base transition-colors hover:bg-black/10"
                              >
                                {t("settings.connections.browse")}
                              </button>
                            </div>
                          </div>
                        )}
                      </section>
                    </div>
                    <div className="flex justify-between border-t border-border-theme px-5 py-3">
                      <button
                        type="button"
                        onClick={() => setManagerMode("list")}
                        className="rounded-full bg-black/5 px-4 py-1.5 text-[13px] text-text-base transition-colors hover:bg-black/10"
                      >
                        {t("remote.backToList")}
                      </button>
                      <button
                        type="submit"
                        disabled={!form.name || !form.host || !form.username}
                        className="rounded-full bg-black px-4 py-1.5 text-[13px] text-white transition-colors hover:bg-gray-800 disabled:opacity-60"
                      >
                        {t("settings.connections.save")}
                      </button>
                    </div>
                  </form>
                </>
              )}
            </div>
          </div>,
          document.body,
        )}
    </aside>
  );
}
